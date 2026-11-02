//! Part 21 writer.

use crate::model::{Param, StepFile};
use crate::strings::encode_string_into;
use std::fmt::Write as _;
use std::io::{self, Write};

/// Formats a real as a Part 21 real literal that parses back to exactly the same `f64`: the
/// shortest round-trip digits, always with a decimal point (`1.`, `-0.`, `0.1`, `1.E-300`,
/// `6.02214076E23`). Infinities are written as `1.E999` / `-1.E999` (which read back as
/// infinities); NaN has no Part 21 form and yields `None`.
pub fn format_real(x: f64) -> Option<String> {
    let mut s = String::new();
    push_real(x, &mut s).then_some(s)
}

fn push_real(x: f64, out: &mut String) -> bool {
    if x.is_nan() {
        return false;
    }
    if x.is_infinite() {
        out.push_str(if x > 0.0 { "1.E999" } else { "-1.E999" });
        return true;
    }
    let s = format!("{x:?}");
    let (mant, exp) = match s.find(['e', 'E']) {
        Some(k) => (&s[..k], Some(&s[k + 1..])),
        None => (s.as_str(), None),
    };
    match mant.find('.') {
        Some(_) => out.push_str(mant.trim_end_matches('0')),
        None => {
            out.push_str(mant);
            out.push('.');
        }
    }
    if let Some(e) = exp {
        out.push('E');
        out.push_str(e);
    }
    true
}

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

fn push_param(p: &Param, out: &mut String) -> io::Result<()> {
    match p {
        Param::Unset => out.push('$'),
        Param::Derived => out.push('*'),
        Param::Integer(i) => {
            let _ = write!(out, "{i}");
        }
        Param::Real(x) => {
            if !push_real(*x, out) {
                return Err(invalid("NaN cannot be written to a STEP file".into()));
            }
        }
        Param::String(s) => {
            out.push('\'');
            encode_string_into(s, out);
            out.push('\'');
        }
        Param::Binary(bits) => {
            if !bits.bytes().all(|b| b == b'0' || b == b'1') {
                return Err(invalid(format!("binary value {bits:?} is not a bit string")));
            }
            let pad = (4 - bits.len() % 4) % 4;
            out.push('"');
            out.push(char::from(b'0' + pad as u8));
            let padded: Vec<u8> = std::iter::repeat_n(b'0', pad).chain(bits.bytes()).collect();
            for nibble in padded.chunks(4) {
                let v = nibble.iter().fold(0u32, |acc, &b| acc * 2 + u32::from(b - b'0'));
                out.push(char::from_digit(v, 16).unwrap_or('0').to_ascii_uppercase());
            }
            out.push('"');
        }
        Param::Enum(e) => {
            check_name(e, "enumeration value")?;
            out.push('.');
            out.push_str(e);
            out.push('.');
        }
        Param::Ref(id) => {
            let _ = write!(out, "#{id}");
        }
        Param::List(items) => {
            out.push('(');
            push_params(items, out)?;
            out.push(')');
        }
        Param::Typed(name, inner) => {
            check_name(name, "type name")?;
            out.push_str(name);
            out.push('(');
            push_param(inner, out)?;
            out.push(')');
        }
    }
    Ok(())
}

fn push_params(items: &[Param], out: &mut String) -> io::Result<()> {
    for (k, it) in items.iter().enumerate() {
        if k > 0 {
            out.push(',');
        }
        push_param(it, out)?;
    }
    Ok(())
}

fn check_name(name: &str, what: &str) -> io::Result<()> {
    let ok = !name.is_empty()
        && name.bytes().enumerate().all(|(k, b)| b.is_ascii_alphanumeric() || b == b'_' || (k == 0 && b == b'!'));
    if ok {
        Ok(())
    } else {
        Err(invalid(format!("{what} {name:?} is not a valid STEP keyword")))
    }
}

impl StepFile {
    /// Writes the file: the header entries as stored, then one DATA section with every instance
    /// (ids kept) in insertion order. Fails on values without a Part 21 form (NaN, malformed
    /// names or bit strings) and on I/O errors.
    pub fn write<W: Write>(&self, mut w: W) -> io::Result<()> {
        let mut line = String::with_capacity(256);
        w.write_all(b"ISO-10303-21;\nHEADER;\n")?;
        for h in &self.header {
            line.clear();
            check_name(&h.entity, "header entity name")?;
            line.push_str(&h.entity);
            line.push('(');
            push_params(&h.params, &mut line)?;
            line.push_str(");\n");
            w.write_all(line.as_bytes())?;
        }
        w.write_all(b"ENDSEC;\nDATA;\n")?;
        for inst in self.instances() {
            line.clear();
            let _ = write!(line, "#{}=", inst.id);
            if inst.is_complex() {
                line.push('(');
                for p in &inst.params {
                    match p {
                        Param::Typed(name, inner) => match inner.as_ref() {
                            Param::List(ps) => {
                                check_name(name, "partial entity name")?;
                                line.push_str(name);
                                line.push('(');
                                push_params(ps, &mut line)?;
                                line.push(')');
                            }
                            _ => return Err(invalid(format!("complex instance #{} has a malformed part", inst.id))),
                        },
                        _ => return Err(invalid(format!("complex instance #{} has a malformed part", inst.id))),
                    }
                }
                line.push_str(");\n");
            } else {
                check_name(&inst.entity, "entity name")?;
                line.push_str(&inst.entity);
                line.push('(');
                push_params(&inst.params, &mut line)?;
                line.push_str(");\n");
            }
            w.write_all(line.as_bytes())?;
        }
        w.write_all(b"ENDSEC;\nEND-ISO-10303-21;\n")?;
        w.flush()
    }
}
