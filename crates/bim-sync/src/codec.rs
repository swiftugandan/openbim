//! The binary encoding of edits, shared by the wire protocol and the store.
//!
//! * integers are little-endian: `u8` tags, `u32` lengths and peers, `u64` counters;
//! * a string is a `u32` byte length and its UTF-8 bytes; an `f64` is its `to_bits()` as `u64`;
//! * a value is a tag, then: `0` Num (`f64`), `1` Text (string), `2` Ref (string), `3` Bool (one
//!   byte, 0 or 1), `4` List (`u32` count, then that many values);
//! * an op is a tag, then: `0` Create (name, kind), `1` Set (name, key, value), `2` Delete (name);
//! * a stamped op is the counter, the peer, then the op.
//!
//! **The decoder is total.** Every length is checked against the bytes that remain, a count never
//! reserves more memory than the input could fill, List nesting is capped, bytes must be valid
//! UTF-8, booleans are 0 or 1, numbers are finite, nothing may trail. It returns an error for any
//! input and never panics. Every input it accepts is the canonical encoding of what it decodes to.

use crate::log::Summary;
use crate::op::{StampedOp, SyncOp, SyncValue, MAX_VALUE_DEPTH};
use crate::Stamp;

const TAG_NUM: u8 = 0;
const TAG_TEXT: u8 = 1;
const TAG_REF: u8 = 2;
const TAG_BOOL: u8 = 3;
const TAG_LIST: u8 = 4;

const OP_CREATE: u8 = 0;
const OP_SET: u8 = 1;
const OP_DELETE: u8 = 2;

/// Fewest bytes an encoded value takes (a Bool: tag and byte).
const MIN_VALUE_BYTES: usize = 2;
/// Fewest bytes an encoded stamped op takes (counter, peer, tag, empty name).
const MIN_STAMPED_BYTES: usize = 8 + 4 + 1 + 4;
/// Bytes of one summary entry: peer, greatest counter, digest.
const SUMMARY_ENTRY_BYTES: usize = 4 + 8 + 8;

pub(crate) fn put_u32(out: &mut Vec<u8>, x: u32) {
    out.extend_from_slice(&x.to_le_bytes());
}

pub(crate) fn put_u64(out: &mut Vec<u8>, x: u64) {
    out.extend_from_slice(&x.to_le_bytes());
}

pub(crate) fn put_len(out: &mut Vec<u8>, n: usize) {
    // A length above u32::MAX cannot be framed (messages are capped far below); saturating keeps
    // the encoder total and the decoder rejects the frame.
    put_u32(out, u32::try_from(n).unwrap_or(u32::MAX));
}

pub(crate) fn put_str(out: &mut Vec<u8>, s: &str) {
    put_len(out, s.len());
    out.extend_from_slice(s.as_bytes());
}

fn put_value(out: &mut Vec<u8>, v: &SyncValue) {
    match v {
        SyncValue::Num(x) => {
            out.push(TAG_NUM);
            put_u64(out, x.to_bits());
        }
        SyncValue::Text(s) => {
            out.push(TAG_TEXT);
            put_str(out, s);
        }
        SyncValue::Ref(s) => {
            out.push(TAG_REF);
            put_str(out, s);
        }
        SyncValue::Bool(b) => {
            out.push(TAG_BOOL);
            out.push(u8::from(*b));
        }
        SyncValue::List(items) => {
            out.push(TAG_LIST);
            put_len(out, items.len());
            for item in items {
                put_value(out, item);
            }
        }
    }
}

/// Appends the encoding of an op.
pub fn put_op(out: &mut Vec<u8>, op: &SyncOp) {
    match op {
        SyncOp::Create { name, kind } => {
            out.push(OP_CREATE);
            put_str(out, name);
            put_str(out, kind);
        }
        SyncOp::Set { name, key, value } => {
            out.push(OP_SET);
            put_str(out, name);
            put_str(out, key);
            put_value(out, value);
        }
        SyncOp::Delete { name } => {
            out.push(OP_DELETE);
            put_str(out, name);
        }
    }
}

/// Appends the encoding of a stamped op.
pub fn put_stamped(out: &mut Vec<u8>, op: &StampedOp) {
    put_u64(out, op.stamp.counter);
    put_u32(out, op.stamp.peer);
    put_op(out, &op.op);
}

/// The encoding of an op alone: what the store keeps in its `op` column.
pub fn op_bytes(op: &SyncOp) -> Vec<u8> {
    let mut out = Vec::new();
    put_op(&mut out, op);
    out
}

/// Appends a summary: a count, then (peer, greatest counter, digest) in ascending peer order.
pub(crate) fn put_summary(out: &mut Vec<u8>, summary: &Summary) {
    put_len(out, summary.vv.len());
    for (&peer, &counter) in &summary.vv {
        put_u32(out, peer);
        put_u64(out, counter);
        put_u64(out, summary.digests.get(&peer).copied().unwrap_or(0));
    }
}

/// Appends a list of stamped ops: a count, then the ops.
pub(crate) fn put_ops(out: &mut Vec<u8>, ops: &[StampedOp]) {
    put_len(out, ops.len());
    for op in ops {
        put_stamped(out, op);
    }
}

/// A cursor over the bytes being decoded. Every read is bounds-checked.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// A reader at the start of `buf`.
    pub fn new(buf: &'a [u8]) -> Reader<'a> {
        Reader { buf, pos: 0 }
    }

    /// Bytes not read yet.
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    /// Fails unless every byte was read.
    pub fn finish(self) -> Result<(), String> {
        match self.remaining() {
            0 => Ok(()),
            n => Err(format!("{n} unexpected trailing byte(s)")),
        }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if n > self.remaining() {
            return Err(format!("truncated: needs {n} byte(s), {} left", self.remaining()));
        }
        let bytes = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(bytes)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn u32(&mut self) -> Result<u32, String> {
        let bytes: [u8; 4] = self.take(4)?.try_into().map_err(|_| "truncated".to_string())?;
        Ok(u32::from_le_bytes(bytes))
    }

    pub(crate) fn u64(&mut self) -> Result<u64, String> {
        let bytes: [u8; 8] = self.take(8)?.try_into().map_err(|_| "truncated".to_string())?;
        Ok(u64::from_le_bytes(bytes))
    }

    pub(crate) fn str(&mut self) -> Result<String, String> {
        let len = self.u32()? as usize;
        let bytes = self.take(len)?;
        std::str::from_utf8(bytes).map(str::to_owned).map_err(|_| "a string is not valid UTF-8".to_string())
    }

    /// A count of items that each take at least `min_bytes`, checked against the bytes left.
    pub(crate) fn count(&mut self, min_bytes: usize) -> Result<usize, String> {
        let n = self.u32()? as usize;
        if n > self.remaining() / min_bytes {
            return Err(format!("a count of {n} cannot fit in the {} byte(s) left", self.remaining()));
        }
        Ok(n)
    }

    fn value(&mut self, depth: usize) -> Result<SyncValue, String> {
        Ok(match self.u8()? {
            TAG_NUM => {
                let x = f64::from_bits(self.u64()?);
                if !x.is_finite() {
                    return Err("a number is not finite".to_string());
                }
                SyncValue::Num(x)
            }
            TAG_TEXT => SyncValue::Text(self.str()?),
            TAG_REF => SyncValue::Ref(self.str()?),
            TAG_BOOL => match self.u8()? {
                0 => SyncValue::Bool(false),
                1 => SyncValue::Bool(true),
                other => return Err(format!("a boolean is {other}, not 0 or 1")),
            },
            TAG_LIST => {
                if depth >= MAX_VALUE_DEPTH {
                    return Err(format!("lists nested deeper than {MAX_VALUE_DEPTH}"));
                }
                let n = self.count(MIN_VALUE_BYTES)?;
                let mut items = Vec::with_capacity(n);
                for _ in 0..n {
                    items.push(self.value(depth + 1)?);
                }
                SyncValue::List(items)
            }
            other => return Err(format!("unknown value tag {other}")),
        })
    }

    /// Reads an op and checks it.
    pub fn op(&mut self) -> Result<SyncOp, String> {
        let op = match self.u8()? {
            OP_CREATE => SyncOp::Create { name: self.str()?, kind: self.str()? },
            OP_SET => {
                let name = self.str()?;
                let key = self.str()?;
                SyncOp::Set { name, key, value: self.value(0)? }
            }
            OP_DELETE => SyncOp::Delete { name: self.str()? },
            other => return Err(format!("unknown op tag {other}")),
        };
        op.check()?;
        Ok(op)
    }

    /// Reads a stamped op and checks it.
    pub fn stamped(&mut self) -> Result<StampedOp, String> {
        let counter = self.u64()?;
        let peer = self.u32()?;
        let op = StampedOp { stamp: Stamp { counter, peer }, op: self.op()? };
        op.check()?;
        Ok(op)
    }

    pub(crate) fn summary(&mut self) -> Result<Summary, String> {
        let n = self.count(SUMMARY_ENTRY_BYTES)?;
        let mut summary = Summary::default();
        let mut last: Option<u32> = None;
        for _ in 0..n {
            let peer = self.u32()?;
            if last.is_some_and(|l| l >= peer) {
                return Err("a version vector is not in strictly ascending peer order".to_string());
            }
            last = Some(peer);
            let counter = self.u64()?;
            if counter == 0 || counter > crate::op::MAX_COUNTER {
                return Err(format!("a version vector holds the out-of-range counter {counter}"));
            }
            summary.vv.insert(peer, counter);
            summary.digests.insert(peer, self.u64()?);
        }
        Ok(summary)
    }

    pub(crate) fn ops(&mut self) -> Result<Vec<StampedOp>, String> {
        let n = self.count(MIN_STAMPED_BYTES)?;
        let mut ops = Vec::with_capacity(n);
        for _ in 0..n {
            ops.push(self.stamped()?);
        }
        Ok(ops)
    }
}

/// Decodes an op alone (the store's `op` column).
pub fn op_from_bytes(bytes: &[u8]) -> Result<SyncOp, String> {
    let mut reader = Reader::new(bytes);
    let op = reader.op()?;
    reader.finish()?;
    Ok(op)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A sample of every op and value shape.
    pub(crate) fn sample_ops() -> Vec<SyncOp> {
        let list = SyncValue::List(vec![
            SyncValue::Num(-0.0),
            SyncValue::Text("ünï cödé\n\"quoted\"".into()),
            SyncValue::Ref("L 1".into()),
            SyncValue::Bool(true),
            SyncValue::Bool(false),
            SyncValue::List(vec![]),
            SyncValue::List(vec![SyncValue::Num(5e-324), SyncValue::List(vec![SyncValue::Num(f64::MAX)])]),
        ]);
        let set = |value| SyncOp::Set { name: "w".into(), key: "k".into(), value };
        vec![
            SyncOp::Create { name: "w".into(), kind: "Wall".into() },
            SyncOp::Create { name: String::new(), kind: "K".into() },
            set(SyncValue::Num(3.5)),
            set(SyncValue::Num(-0.0)),
            set(SyncValue::Num(f64::MIN_POSITIVE / 4.0)),
            set(SyncValue::Text(String::new())),
            set(SyncValue::Ref("ghost".into())),
            set(SyncValue::Bool(true)),
            set(list),
            SyncOp::Delete { name: "w".into() },
            SyncOp::Delete { name: String::new() },
        ]
    }

    #[test]
    fn every_op_round_trips_bit_for_bit() {
        for op in sample_ops() {
            let bytes = op_bytes(&op);
            let back = op_from_bytes(&bytes).unwrap();
            assert_eq!(back, op);
            assert_eq!(op_bytes(&back), bytes, "canonical");
        }
        // Bit patterns survive: -0.0 stays -0.0 and a subnormal stays itself.
        let SyncOp::Set { value: SyncValue::Num(x), .. } = op_from_bytes(&op_bytes(&sample_ops()[3])).unwrap() else { panic!() };
        assert_eq!(x.to_bits(), (-0.0f64).to_bits());
    }

    #[test]
    fn every_truncation_and_every_extension_is_an_error() {
        for op in sample_ops() {
            let bytes = op_bytes(&op);
            for cut in 0..bytes.len() {
                assert!(op_from_bytes(&bytes[..cut]).is_err(), "{op:?} cut at {cut}");
            }
            let mut longer = bytes.clone();
            longer.push(0);
            assert!(op_from_bytes(&longer).is_err(), "trailing byte accepted for {op:?}");
        }
    }

    #[test]
    fn flipped_bits_never_panic_and_accepted_frames_are_canonical() {
        let mut seed = 0xDEAD_BEEF_u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for op in sample_ops() {
            let bytes = op_bytes(&op);
            for _ in 0..400 {
                let mut broken = bytes.clone();
                for _ in 0..=(next() % 3) {
                    let i = (next() as usize) % broken.len();
                    broken[i] ^= 1 << (next() % 8);
                }
                if let Ok(decoded) = op_from_bytes(&broken) {
                    assert_eq!(op_bytes(&decoded), broken, "an accepted input must be canonical");
                    assert!(decoded.check().is_ok());
                }
            }
        }
    }

    #[test]
    fn hostile_counts_and_lengths_are_rejected_without_allocating() {
        // A list that claims 4 billion items in a handful of bytes.
        let mut bytes = vec![OP_SET];
        put_str(&mut bytes, "w");
        put_str(&mut bytes, "k");
        bytes.push(TAG_LIST);
        put_u32(&mut bytes, u32::MAX);
        assert!(op_from_bytes(&bytes).unwrap_err().contains("cannot fit"));
        // A string that claims more bytes than there are.
        let mut bytes = vec![OP_DELETE];
        put_u32(&mut bytes, u32::MAX);
        assert!(op_from_bytes(&bytes).unwrap_err().contains("truncated"));
        // Invalid UTF-8, a boolean of 2, an unknown tag, a NaN, an infinity, an empty kind.
        let mut bytes = vec![OP_DELETE];
        put_u32(&mut bytes, 1);
        bytes.push(0xff);
        assert!(op_from_bytes(&bytes).unwrap_err().contains("UTF-8"));
        let head = |value: &[u8]| {
            let mut bytes = vec![OP_SET];
            put_str(&mut bytes, "w");
            put_str(&mut bytes, "k");
            bytes.extend_from_slice(value);
            bytes
        };
        assert!(op_from_bytes(&head(&[TAG_BOOL, 2])).unwrap_err().contains("not 0 or 1"));
        assert!(op_from_bytes(&head(&[9])).unwrap_err().contains("unknown value tag"));
        let mut nan = vec![TAG_NUM];
        put_u64(&mut nan, f64::NAN.to_bits());
        assert!(op_from_bytes(&head(&nan)).unwrap_err().contains("not finite"));
        let mut inf = vec![TAG_NUM];
        put_u64(&mut inf, f64::INFINITY.to_bits());
        assert!(op_from_bytes(&head(&inf)).is_err());
        assert!(op_from_bytes(&[7]).unwrap_err().contains("unknown op tag"));
        let mut empty_kind = vec![OP_CREATE];
        put_str(&mut empty_kind, "w");
        put_str(&mut empty_kind, "");
        assert!(op_from_bytes(&empty_kind).unwrap_err().contains("needs a kind"));
    }

    #[test]
    fn list_nesting_is_capped_and_cannot_overflow_the_stack() {
        let nested = |levels: usize| {
            let mut bytes = vec![OP_SET];
            put_str(&mut bytes, "w");
            put_str(&mut bytes, "k");
            for _ in 0..levels {
                bytes.push(TAG_LIST);
                put_u32(&mut bytes, 1);
            }
            bytes.extend_from_slice(&[TAG_BOOL, 1]);
            bytes
        };
        assert!(op_from_bytes(&nested(MAX_VALUE_DEPTH)).is_ok());
        assert!(op_from_bytes(&nested(MAX_VALUE_DEPTH + 1)).unwrap_err().contains("nested deeper"));
        // Far deeper than any stack could take if the decoder recursed without a cap.
        assert!(op_from_bytes(&nested(200_000)).is_err());
    }

    #[test]
    fn stamped_ops_check_their_counter() {
        let mut bytes = Vec::new();
        put_stamped(&mut bytes, &StampedOp { stamp: Stamp { counter: 4, peer: 9 }, op: SyncOp::Delete { name: "a".into() } });
        let back = Reader::new(&bytes).stamped().unwrap();
        assert_eq!((back.stamp.counter, back.stamp.peer), (4, 9));
        let mut zero = Vec::new();
        put_u64(&mut zero, 0);
        put_u32(&mut zero, 1);
        put_op(&mut zero, &SyncOp::Delete { name: "a".into() });
        assert!(Reader::new(&zero).stamped().is_err());
        let mut huge = Vec::new();
        put_u64(&mut huge, u64::MAX);
        put_u32(&mut huge, 1);
        put_op(&mut huge, &SyncOp::Delete { name: "a".into() });
        assert!(Reader::new(&huge).stamped().is_err());
    }
}
