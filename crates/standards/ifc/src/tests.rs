use crate::guid;
use crate::{decode_string, encode_string, format_real, ErrorKind, Param, Schema, StepFile};

const SAMPLE: &str = "ISO-10303-21;
HEADER;
/* header comment */
FILE_DESCRIPTION(('ViewDefinition [CoordinationView]'),'2;1');
FILE_NAME('a.ifc','2024-01-01T00:00:00',('Author'),(''),'pre','sys','');
FILE_SCHEMA(('IFC4'));
ENDSEC;
DATA;
#1=IFCPROJECT('0fTIQyfUP3xwnpaRIwq4Zs',$,'Projekt \\X2\\00C4\\X0\\',$,$,$,$,(#3),$);
#2=IFCSIUNIT(*,.LENGTHUNIT.,.MILLI.,.METRE.);
#3 =
  /* split ; ' ( */ IFCCARTESIANPOINT((0.,-0.,1.E-300));
#4=IFCPROPERTYSINGLEVALUE('it''s \\\\ x',$,IFCLOGICAL(.U.),$);
#5=IFCBLOBTEXTURE(.T.,.F.,'M',$,$,'PNG',\"05B1A\");
#6=IFCPIXELTEXTURE(.T.,.T.,$,$,$,2,1,3,(\"0B0F0F0\",\"30\",\"0\"));
#7=IFCPERSON($,'\\X4\\0001F600\\X0\\ \\X2\\D83DDE00\\X0\\',$,$,$,$,$,$);
#8=IFCPROPERTYSINGLEVALUE('n',$,IFCINTEGER(-42),$);
#9=(IFCA(1,2.5)IFCB('x'));
ENDSEC;
END-ISO-10303-21;
";

fn sample() -> StepFile {
    StepFile::parse_str(SAMPLE).expect("sample parses")
}

#[test]
fn parses_values() {
    let f = sample();
    assert_eq!(f.len(), 9);
    assert_eq!(f.header.len(), 3);
    let p = f.get(1).unwrap();
    assert_eq!(p.entity, "IFCPROJECT");
    assert_eq!(p.params[2], Param::String("Projekt Ä".into()));
    assert_eq!(p.params[7], Param::List(vec![Param::Ref(3)]));
    assert_eq!(f.get(2).unwrap().params[0], Param::Derived);
    assert_eq!(f.get(2).unwrap().params[1], Param::Enum("LENGTHUNIT".into()));
    let pt = &f.get(3).unwrap().params[0];
    let xs = pt.as_list().unwrap();
    assert_eq!(xs[0], Param::Real(0.0));
    assert!(matches!(xs[1], Param::Real(z) if z == 0.0 && z.is_sign_negative()));
    assert_eq!(xs[2], Param::Real(1e-300));
    assert_eq!(f.get(4).unwrap().params[0], Param::String("it's \\ x".into()));
    assert_eq!(f.get(4).unwrap().params[2], Param::Typed("IFCLOGICAL".into(), Box::new(Param::Enum("U".into()))));
    assert_eq!(f.get(5).unwrap().params[6], Param::Binary("0101101100011010".into()));
    let bins = f.get(6).unwrap().params[8].as_list().unwrap().to_vec();
    assert_eq!(bins, vec![Param::Binary("101100001111000011110000".into()), Param::Binary("0".into()), Param::Binary("".into())]);
    assert_eq!(f.get(7).unwrap().params[1], Param::String("😀 😀".into()));
    assert_eq!(f.get(8).unwrap().params[2].as_i64(), Some(-42));
    let c = f.get(9).unwrap();
    assert!(c.is_complex());
    let parts = c.complex_parts();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].0, "IFCA");
    assert_eq!(parts[0].1, &[Param::Integer(1), Param::Real(2.5)][..]);
}

#[test]
fn round_trip_is_identical() {
    let f = sample();
    let text = f.to_step_string();
    let g = StepFile::parse_str(&text).expect("own output parses");
    assert_eq!(f.header, g.header);
    let a: Vec<_> = f.instances().cloned().collect();
    let b: Vec<_> = g.instances().cloned().collect();
    // compare reals by bits (-0.0 == 0.0 under PartialEq)
    assert_eq!(format!("{a:?}"), format!("{b:?}"));
    assert_eq!(text, g.to_step_string());
    assert!(text.contains("#9=(IFCA(1,2.5)IFCB('x'));"));
    assert!(text.contains("(0.,-0.,1.E-300)"));
    assert!(text.contains("\"05B1A\""));
}

#[test]
fn reals_round_trip_bit_exact() {
    let xs = [
        0.0,
        -0.0,
        1e-300,
        5e-324,
        f64::MAX,
        f64::MIN_POSITIVE,
        -12345678.123456789,
        0.1,
        1.0 / 3.0,
        2.5e-7,
        6.02214076e23,
        -1.0,
        100.0,
        1e16,
        123456789012345680.0,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ];
    for x in xs {
        let s = format_real(x).unwrap();
        assert!(s.contains('.'), "{s}");
        let src = format!("ISO-10303-21;HEADER;ENDSEC;DATA;#1=X({s});ENDSEC;END-ISO-10303-21;");
        let f = StepFile::parse_str(&src).unwrap();
        match f.get(1).unwrap().params[0] {
            Param::Real(y) => assert_eq!(y.to_bits(), x.to_bits(), "{s}"),
            ref p => panic!("{p:?}"),
        }
    }
    assert_eq!(format_real(100.0).unwrap(), "100.");
    assert_eq!(format_real(-0.0).unwrap(), "-0.");
    assert_eq!(format_real(1e-300).unwrap(), "1.E-300");
    assert!(format_real(f64::NAN).is_none());
    let mut f = StepFile::new(vec![]);
    f.add("X", vec![Param::Real(f64::NAN)]);
    assert!(f.write(Vec::new()).is_err());
}

#[test]
fn integers_and_reals_stay_distinct() {
    let f = StepFile::parse_str("ISO-10303-21;HEADER;ENDSEC;DATA;#1=X(7,7.,+3,-0,1E2);ENDSEC;END-ISO-10303-21;").unwrap();
    let p = &f.get(1).unwrap().params;
    assert_eq!(p[0], Param::Integer(7));
    assert_eq!(p[1], Param::Real(7.0));
    assert_eq!(p[2], Param::Integer(3));
    assert_eq!(p[3], Param::Integer(0));
    assert_eq!(p[4], Param::Real(100.0));
    assert!(f.to_step_string().contains("#1=X(7,7.,3,0,100.);"));
}

#[test]
fn string_escapes() {
    assert_eq!(decode_string(b"a''b").unwrap(), "a'b");
    assert_eq!(decode_string(b"c\\\\d").unwrap(), "c\\d");
    assert_eq!(decode_string(b"\\X2\\00C400E9\\X0\\x").unwrap(), "Äéx");
    assert_eq!(decode_string(b"\\X2\\D83DDE00\\X0\\").unwrap(), "😀");
    assert_eq!(decode_string(b"\\X4\\0001F3D700004E2D\\X0\\").unwrap(), "🏗中");
    assert_eq!(decode_string(b"\\X\\E9t\\X\\C4").unwrap(), "étÄ");
    assert_eq!(decode_string(b"\\S\\Dx").unwrap(), "Äx");
    assert_eq!(decode_string(b"\\PA\\\\S\\D").unwrap(), "Ä");
    assert_eq!(decode_string("raw ü".as_bytes()).unwrap(), "raw ü");
    assert_eq!(decode_string(b"latin \xfc").unwrap(), "latin ü");
    assert_eq!(decode_string(b"C:\\temp").unwrap(), "C:\\temp");
    assert!(decode_string(b"\\X2\\00C4").is_err());
    assert!(decode_string(b"\\X2\\00C\\X0\\").is_err());
    assert!(decode_string(b"\\X2\\D83D\\X0\\").is_err());
    assert!(decode_string(b"\\X4\\FFFFFFFF\\X0\\").is_err());
    // \S\ followed by an apostrophe inside a literal
    let f = StepFile::parse_str("ISO-10303-21;HEADER;ENDSEC;DATA;#1=X('a\\S\\'b');ENDSEC;END-ISO-10303-21;").unwrap();
    assert_eq!(f.get(1).unwrap().params[0], Param::String("a§b".into()));
    // raw line breaks inside a literal are part of the value (as ifcopenshell reads them)
    let f = StepFile::parse_str("ISO-10303-21;HEADER;ENDSEC;DATA;#1=X('ab\r\ncd');ENDSEC;END-ISO-10303-21;").unwrap();
    assert_eq!(f.get(1).unwrap().params[0], Param::String("ab\r\ncd".into()));
    assert!(f.to_step_string().contains("'ab\\X2\\000D000A\\X0\\cd'"));
}

#[test]
fn string_encoding_round_trips() {
    for s in ["", "'", "''", "\\", "a'b\\c", "\t-tab", "line1\nline2 é", "Ünïcödé 中文 😀 🏗", "x\u{7f}y", "\u{FFFF}\u{10FFFF}"] {
        let enc = encode_string(s);
        assert!(enc.bytes().all(|b| (0x20..0x7f).contains(&b)), "{enc}");
        assert_eq!(decode_string(enc.as_bytes()).unwrap(), s);
    }
    assert_eq!(encode_string("Ä中😀a"), "\\X2\\00C44E2D\\X0\\\\X4\\0001F600\\X0\\a");
}

#[test]
fn binary_round_trips() {
    for bits in ["", "1", "01", "101", "1011", "10110", "0000000"] {
        let mut f = StepFile::new(vec![]);
        f.add("X", vec![Param::Binary(bits.into())]);
        let g = StepFile::parse_str(&f.to_step_string()).unwrap();
        assert_eq!(g.get(1).unwrap().params[0], Param::Binary(bits.into()));
    }
    let mut f = StepFile::new(vec![]);
    f.add("X", vec![Param::Binary("012".into())]);
    assert!(f.write(Vec::new()).is_err());
}

fn reject(src: &str) -> ErrorKind {
    match StepFile::parse_str(src) {
        Ok(_) => panic!("accepted {src:?}"),
        Err(e) => {
            assert!(!e.to_string().is_empty());
            e.kind
        }
    }
}

#[test]
fn malformed_input_is_an_error() {
    let h = "ISO-10303-21;HEADER;FILE_SCHEMA(('IFC4'));ENDSEC;DATA;";
    assert_eq!(reject(""), ErrorKind::UnexpectedEof);
    assert_eq!(reject("hello, this is not ISO 10303-21\n"), ErrorKind::UnexpectedToken);
    assert_eq!(reject(&format!("{h}#1=X(1);")), ErrorKind::UnexpectedEof);
    assert_eq!(reject(&format!("{h}#1=X(1);ENDSEC;")), ErrorKind::UnexpectedEof);
    assert_eq!(reject(&format!("{h}#1=X('abc,$);ENDSEC;END-ISO-10303-21;")), ErrorKind::UnterminatedString);
    assert_eq!(reject(&format!("{h}#1=X(1)(;ENDSEC;END-ISO-10303-21;")), ErrorKind::UnexpectedToken);
    assert_eq!(reject(&format!("{h}#1=X((1);ENDSEC;END-ISO-10303-21;")), ErrorKind::UnexpectedToken);
    assert_eq!(reject(&format!("{h}#1=X(1));ENDSEC;END-ISO-10303-21;")), ErrorKind::UnexpectedToken);
    assert_eq!(reject(&format!("{h}#1=X(1);#1=X(2);ENDSEC;END-ISO-10303-21;")), ErrorKind::DuplicateId);
    assert_eq!(reject(&format!("{h}/* open comment")), ErrorKind::UnterminatedComment);
    assert_eq!(reject(&format!("{h}#1=X(99999999999999999999);ENDSEC;END-ISO-10303-21;")), ErrorKind::InvalidNumber);
    assert_eq!(reject(&format!("{h}#1=X(\"4F\");ENDSEC;END-ISO-10303-21;")), ErrorKind::InvalidBinary);
    assert_eq!(reject(&format!("{h}#1=X('\\X2\\00');ENDSEC;END-ISO-10303-21;")), ErrorKind::InvalidEscape);
    assert_eq!(reject(&format!("{h}#1=X(.A);ENDSEC;END-ISO-10303-21;")), ErrorKind::InvalidToken);
    assert_eq!(reject("ISO-10303-21;HEADER;FILE_SCHEMA(('IFC4'));"), ErrorKind::UnexpectedEof);
    let deep = format!("{h}#1=X({}1{});ENDSEC;END-ISO-10303-21;", "(".repeat(100_000), ")".repeat(100_000));
    assert_eq!(reject(&deep), ErrorKind::TooDeep);
}

#[test]
fn every_truncation_is_an_error_without_panic() {
    let bytes = SAMPLE.as_bytes();
    let end = SAMPLE.rfind("END-ISO").unwrap();
    for cut in 0..end {
        assert!(StepFile::read(&bytes[..cut]).is_err(), "prefix of {cut} bytes accepted");
    }
}

#[test]
fn arbitrary_bytes_do_not_panic() {
    let mut state = 12345u64;
    let alphabet = b"#=();,'\"$*.\\XS2P0A9-+E/ ISOHEADRNC_1T\n";
    for _ in 0..2000 {
        let mut s = b"ISO-10303-21;HEADER;ENDSEC;DATA;".to_vec();
        for _ in 0..60 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            s.push(alphabet[(state >> 33) as usize % alphabet.len()]);
        }
        let _ = StepFile::read(&s[..]);
    }
}

#[test]
fn multiple_data_sections_and_trailing_content() {
    let f = StepFile::parse_str(
        "ISO-10303-21;HEADER;ENDSEC;DATA;#1=A();ENDSEC;DATA('x');#2=B(#1);ENDSEC;END-ISO-10303-21;\ntrailing",
    )
    .unwrap();
    assert_eq!(f.len(), 2);
    assert_eq!(f.get(1).unwrap().params, vec![]);
    assert_eq!(f.max_id(), 2);
}

#[test]
fn keywords_are_upper_cased() {
    let f = StepFile::parse_str("ISO-10303-21;HEADER;ENDSEC;DATA;#1=IfcLabel(.t.,IfcText('x'));ENDSEC;END-ISO-10303-21;").unwrap();
    let i = f.get(1).unwrap();
    assert_eq!(i.entity, "IFCLABEL");
    assert_eq!(i.params[0], Param::Enum("T".into()));
    assert_eq!(i.params[1].as_typed().unwrap().0, "IFCTEXT");
}

#[test]
fn guid_vectors() {
    assert_eq!(guid::compress(0xff), "000000000000000000003$");
    assert_eq!(guid::compress(u128::MAX), "3$$$$$$$$$$$$$$$$$$$$$");
    assert_eq!(guid::compress_hex("0123456789abcdef0123456789abcdef").unwrap(), "018qLdYQlDxm4ZHMU9gytl");
    assert_eq!(guid::expand_hex("018qLdYQlDxm4ZHMU9gytl").unwrap(), "0123456789abcdef0123456789abcdef");
    assert_eq!(guid::expand("3$$$$$$$$$$$$$$$$$$$$$"), Some(u128::MAX));
    assert_eq!(guid::expand("4$$$$$$$$$$$$$$$$$$$$$"), None);
    assert_eq!(guid::expand("short"), None);
    assert_eq!(guid::expand("018qLdYQlDxm4ZHMU9gyt!"), None);
    let mut g = guid::GuidGenerator::new(7);
    let a = g.next_guid();
    let b = g.next_guid();
    assert_ne!(a, b);
    assert_eq!(guid::compress(guid::expand(&a).unwrap()), a);
}

#[test]
fn typed_access_and_builder() {
    let f = sample();
    assert_eq!(f.schema(), Schema::Ifc4);
    let p = f.entity(1).unwrap();
    assert_eq!(p.type_name(), "IfcProject");
    assert_eq!(p.get_str("Name"), Some("Projekt Ä"));
    assert_eq!(p.get_str("GlobalId"), Some("0fTIQyfUP3xwnpaRIwq4Zs"));
    assert_eq!(p.get_refs("RepresentationContexts"), vec![3]);
    assert!(p.get("UnitsInContext").is_none());
    assert!(p.is_a("IfcRoot") && p.is_a("IfcContext") && !p.is_a("IfcProduct"));
    assert_eq!(f.entity(3).unwrap().get_f64s("Coordinates").len(), 3);
    assert_eq!(f.attr(4, "Name").and_then(Param::as_str), Some("it's \\ x"));

    let mut g = StepFile::with_schema("IFC4", "x.ifc", "2024-01-01T00:00:00", "bim");
    let pt = g.add_entity("IfcCartesianPoint", &[("Coordinates", Param::reals([1.0, 2.0, 3.0]))]).unwrap();
    let pl = g.add_entity("IfcAxis2Placement3D", &[("Location", Param::Ref(pt))]).unwrap();
    let w = g
        .add_entity(
            "IfcWall",
            &[("GlobalId", Param::string(guid::compress(42))), ("Name", Param::string("W")), ("ObjectPlacement", Param::Ref(pl))],
        )
        .unwrap();
    assert_eq!((pt, pl, w), (1, 2, 3));
    assert_eq!(g.get(w).unwrap().params.len(), 9);
    assert!(g.add_entity("IfcWall", &[("Nope", Param::Unset)]).is_err());
    assert!(g.add_entity("IfcNotAThing", &[]).is_err());
    g.set_attr(w, "Tag", Param::string("T1")).unwrap();
    let text = g.to_step_string();
    let h = StepFile::parse_str(&text).unwrap();
    assert_eq!(h.schema_identifiers(), vec!["IFC4".to_string()]);
    let wall = h.entities_of("IfcBuildingElement").next().unwrap();
    assert_eq!(wall.id(), 3);
    assert_eq!(wall.get_str("Tag"), Some("T1"));
    assert_eq!(wall.get_entity("ObjectPlacement").unwrap().get_entity("Location").unwrap().get_f64s("Coordinates"), vec![1.0, 2.0, 3.0]);
    assert_eq!(h.instances_of("ifcwall").count(), 1);

    assert_eq!(Schema::Ifc2x3.attribute_index("IfcWallStandardCase", "Tag"), Some(7));
    assert_eq!(Schema::Ifc4.attribute_index("IfcWall", "PredefinedType"), Some(8));
    assert!(Schema::Ifc2x3.is_subtype_of("IfcWallStandardCase", "IfcBuildingElement"));
    assert_eq!(Schema::from_identifier("IFC2X3"), Some(Schema::Ifc2x3));
    assert_eq!(Schema::from_identifier("IFC4X3_ADD2"), Some(Schema::Ifc4));
}

#[test]
fn ifc4_standard_and_elemented_cases_are_subtypes_of_their_class() {
    for (sub, parent) in [
        ("IfcColumnStandardCase", "IfcColumn"),
        ("IfcBeamStandardCase", "IfcBeam"),
        ("IfcDoorStandardCase", "IfcDoor"),
        ("IfcWindowStandardCase", "IfcWindow"),
        ("IfcMemberStandardCase", "IfcMember"),
        ("IfcPlateStandardCase", "IfcPlate"),
        ("IfcWallElementedCase", "IfcWall"),
        ("IfcSlabElementedCase", "IfcSlab"),
        ("IfcSlabStandardCase", "IfcSlab"),
        ("IfcWallStandardCase", "IfcWall"),
        ("IfcOpeningStandardCase", "IfcOpeningElement"),
    ] {
        let def = Schema::Ifc4.entity(sub).unwrap_or_else(|| panic!("{sub} missing"));
        assert_eq!(def.attributes, Schema::Ifc4.entity(parent).unwrap().attributes, "{sub}");
        assert!(Schema::Ifc4.is_subtype_of(&sub.to_ascii_uppercase(), parent), "{sub}");
        assert!(Schema::Ifc4.is_subtype_of(sub, "IfcElement"), "{sub}");
    }
    // A `…StandardCase` / `…ElementedCase` name outside the tables is its base class's subtype.
    assert!(Schema::Ifc4.is_subtype_of("IFCRAILINGSTANDARDCASE", "IfcRailing"));
    assert!(Schema::Ifc2x3.is_subtype_of("IfcColumnStandardCase", "IfcBuildingElement"));
    assert!(!Schema::Ifc4.is_subtype_of("STANDARDCASE", "IfcRoot"));
    assert!(!Schema::Ifc4.is_subtype_of("IfcNothingStandardCase", "IfcRoot"));
    // An IFC4 file's IfcDoorStandardCase is an IfcDoor through the typed API.
    let f = StepFile::parse_str(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((''),'2;1');\nFILE_NAME('','',(''),(''),'','','');\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n#1=IFCDOORSTANDARDCASE('0aaaaaaaaaaaaaaaaaaaaa',$,'D',$,$,$,$,$,2.1,0.9,$,$,$);\nENDSEC;\nEND-ISO-10303-21;\n",
    )
    .unwrap();
    let d = f.entities_of("IfcDoor").next().unwrap();
    assert_eq!((d.type_name(), d.get_f64("OverallWidth")), ("IfcDoorStandardCase", Some(0.9)));
}

#[test]
fn insert_rejects_duplicate_ids() {
    let mut f = StepFile::new(vec![]);
    f.insert(crate::Instance::new(10, "x", vec![])).unwrap();
    assert!(f.insert(crate::Instance::new(10, "Y", vec![])).is_err());
    assert_eq!(f.add("Z", vec![]), 11);
    assert_eq!(f.get(10).unwrap().entity, "X");
}
