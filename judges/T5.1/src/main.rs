// External judge driver for T5.1 (bim-ifc STEP reader/writer). Builders must not edit judges/.
// Usage:
//   judge-t5-1 roundtrip <in.ifc> <out.ifc>   read with bim-ifc, write back; exit 0
//   judge-t5-1 dump <in.ifc> <out.jsonl>      one JSON line per parsed instance (bim-ifc's view)
//   judge-t5-1 reject <in.ifc>                exit 0 iff bim-ifc returns Err (never panics)
use bim_ifc::{Param, StepFile};
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::process::exit;

fn read(path: &str) -> StepFile {
    let input = BufReader::new(File::open(path).expect("open input"));
    match StepFile::read(input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bim-ifc rejected {path}: {e:?}");
            exit(1);
        }
    }
}

fn json_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn param(p: &Param, out: &mut String) {
    match p {
        Param::Unset => out.push_str("null"),
        Param::Derived => out.push_str("{\"d\":1}"),
        Param::Integer(i) => out.push_str(&format!("{{\"i\":{i}}}")),
        Param::Real(r) => out.push_str(&format!("{{\"r\":\"{:016x}\"}}", r.to_bits())),
        Param::String(s) => {
            out.push_str("{\"s\":");
            json_str(s, out);
            out.push('}');
        }
        Param::Binary(b) => {
            out.push_str("{\"b\":");
            json_str(b, out);
            out.push('}');
        }
        Param::Enum(e) => {
            out.push_str("{\"e\":");
            json_str(e, out);
            out.push('}');
        }
        Param::Ref(id) => out.push_str(&format!("{{\"ref\":{id}}}")),
        Param::List(items) => {
            out.push('[');
            for (k, it) in items.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                param(it, out);
            }
            out.push(']');
        }
        Param::Typed(name, inner) => {
            out.push_str("{\"t\":");
            json_str(name, out);
            out.push_str(",\"v\":");
            param(inner, out);
            out.push('}');
        }
        // Any further variant the crate may add is reported (and fails the comparison) instead of
        // breaking the build of this judge.
        #[allow(unreachable_patterns)]
        _ => out.push_str("{\"unknown\":1}"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("roundtrip") if args.len() == 4 => {
            let step = read(&args[2]);
            let mut out = BufWriter::new(File::create(&args[3]).expect("create output"));
            if let Err(e) = step.write(&mut out) {
                eprintln!("bim-ifc write failed: {e:?}");
                exit(1);
            }
            out.flush().expect("flush");
        }
        Some("dump") if args.len() == 4 => {
            let step = read(&args[2]);
            let mut out = BufWriter::new(File::create(&args[3]).expect("create output"));
            for inst in step.instances() {
                let mut line = format!("{{\"id\":{},\"entity\":", inst.id);
                json_str(&inst.entity, &mut line);
                line.push_str(",\"params\":[");
                for (k, p) in inst.params.iter().enumerate() {
                    if k > 0 {
                        line.push(',');
                    }
                    param(p, &mut line);
                }
                line.push_str("]}\n");
                out.write_all(line.as_bytes()).expect("write dump");
            }
            out.flush().expect("flush");
        }
        Some("reject") if args.len() == 3 => {
            let input = BufReader::new(File::open(&args[2]).expect("open input"));
            if StepFile::read(input).is_ok() {
                eprintln!("bim-ifc accepted malformed file {}", args[2]);
                exit(1);
            }
        }
        _ => {
            eprintln!("usage: judge-t5-1 roundtrip <in> <out> | dump <in> <out> | reject <in>");
            exit(2);
        }
    }
}
