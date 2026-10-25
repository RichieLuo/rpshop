use super::*;

fn int(out: &mut Vec<u8>, n: i32) {
    out.extend_from_slice(&n.to_le_bytes());
}

fn fixture() -> Vec<u8> {
    let mut out = Vec::new();
    for n in [27, 11, 0, 0, 0, 50001, 31, 3] {
        int(&mut out, n);
    }
    for text in ["Axure:Page", "name", "页面一"] {
        int(&mut out, text.len() as i32);
        out.extend_from_slice(text.as_bytes());
    }
    int(&mut out, 30);
    int(&mut out, 1);
    out.extend_from_slice(&[
        0x78, 0x56, 0x34, 0x12, 0xbc, 0x9a, 0xf0, 0xde, 1, 2, 3, 4, 5, 6, 7, 8,
    ]);
    for n in [26, 15, 1, 18, 1, 9, 1, 1, 1, 1, 8, 2, 8, 3, 28] {
        int(&mut out, n);
    }
    out
}

#[test]
fn reads_real_schema_and_dotnet_guid_byte_order() {
    let graph = parse(&fixture()).unwrap();
    assert_eq!(graph.root, "123456789abcdef00102030405060708");
    assert_eq!(graph.root().unwrap().kind, "Axure:Page");
    assert_eq!(graph.root().unwrap().properties["name"], "页面一");
}

#[test]
fn rejects_truncated_data_and_invalid_table_references() {
    let data = fixture();
    for len in [0, 4, 32, data.len() - 1] {
        assert!(parse(&data[..len]).is_err());
    }
    let mut invalid = data;
    let len = invalid.len();
    invalid[len - 8..len - 4].copy_from_slice(&99_i32.to_le_bytes());
    assert!(parse(&invalid).is_err());
}

#[test]
fn native_write_preserves_bytes_and_appends_edited_strings() {
    let original = fixture();
    let mut graph = parse(&original).unwrap();
    assert!(encode(&graph).unwrap() == original);
    let root = graph.root.clone();
    graph.objects.get_mut(&root).unwrap().properties["name"] = "修改后的页面".into();
    let saved = encode(&graph).unwrap();
    let reopened = parse(&saved).unwrap();
    assert_eq!(reopened.root().unwrap().properties["name"], "修改后的页面");
    assert!(encode(&reopened).unwrap() == saved);
}

fn typed_fixture() -> Vec<u8> {
    let strings = [
        "Axure:Page",
        "f32",
        "f64",
        "long",
        "nan",
        "typed",
        "enum value",
        "zero",
        "bool",
        "binary",
        "ref",
    ];
    let mut out = Vec::new();
    for n in [27, 11, 0, 0, 0, 50001, 31, strings.len() as i32] {
        int(&mut out, n);
    }
    for text in strings {
        int(&mut out, text.len() as i32);
        out.extend_from_slice(text.as_bytes());
    }
    for n in [30, 1] {
        int(&mut out, n);
    }
    out.extend_from_slice(&[1; 16]);
    for n in [26, 15, 1, 18, 1, 9, 1, 1, 1, 9] {
        int(&mut out, n);
    }
    for n in [8, 2, 7 | 0x40] {
        int(&mut out, n);
    }
    out.extend_from_slice(&1.25_f32.to_le_bytes());
    for n in [8, 3, 6] {
        int(&mut out, n);
    }
    out.extend_from_slice(&1.25_f64.to_le_bytes());
    for n in [8, 4, 29] {
        int(&mut out, n);
    }
    out.extend_from_slice(&(i64::MAX - 11).to_le_bytes());
    for n in [8, 5, 6] {
        int(&mut out, n);
    }
    out.extend_from_slice(&0x7ff8_0000_0000_0051_u64.to_le_bytes());
    for n in [8, 6, 1, 2, 5, 42, 8, 7, 17, 1, 25] {
        int(&mut out, n);
    }
    out.push(2); // Preserve noncanonical true bits as well as typed map keys.
    for n in [8, 8, 6] {
        int(&mut out, n);
    }
    out.extend_from_slice(&(-0.0_f64).to_le_bytes());
    for n in [8, 9, 25] {
        int(&mut out, n);
    }
    out.push(2);
    for n in [8, 10, 4, 3] {
        int(&mut out, n);
    }
    out.extend_from_slice(&[0, 7, 255]);
    for n in [8, 11, 3 | 0x20, 1, 28] {
        int(&mut out, n);
    }
    out
}

#[test]
fn preserves_numeric_widths_flags_nonfinite_bits_and_typed_keys_when_editing() {
    let original = typed_fixture();
    let mut graph = parse(&original).unwrap();
    assert!(encode(&graph).unwrap() == original);
    let root = graph.root.clone();
    let properties = &mut graph.objects.get_mut(&root).unwrap().properties;
    properties["f32"] = json!(3.5);
    properties["typed"]["42"] = "edited enum value".into();
    properties["new-property"] = json!({"nested":[true,"新增",1234567890123_i64]});
    let saved = encode(&graph).unwrap();
    let reopened = parse(&saved).unwrap();
    assert_eq!(reopened.root().unwrap().properties["f32"], json!(3.5));
    assert_eq!(
        reopened.root().unwrap().properties["long"],
        json!(i64::MAX - 11)
    );
    assert_eq!(
        reopened.root().unwrap().properties["typed"]["42"],
        "edited enum value"
    );
    assert!(
        saved
            .windows(8)
            .any(|w| w == 0x7ff8_0000_0000_0051_u64.to_le_bytes())
    );
    let layout = &reopened.encoding.as_ref().unwrap().records[&root].value;
    let ValueLayout::Dictionary { entries, .. } = layout else {
        panic!("dictionary");
    };
    let f32_entry = entries
        .iter()
        .find(|entry| entry.json_key == "f32")
        .unwrap();
    assert!(
        matches!(f32_entry.value_layout, ValueLayout::Scalar { token, .. } if token == (7 | 0x40))
    );
    let typed_entry = entries
        .iter()
        .find(|entry| entry.json_key == "typed")
        .unwrap();
    let ValueLayout::Dictionary { entries, .. } = &typed_entry.value_layout else {
        panic!("dictionary");
    };
    assert!(matches!(
        entries[0].key_layout,
        ValueLayout::Scalar { token: 5, .. }
    ));
    assert!(encode(&reopened).unwrap() == saved);
}

#[test]
fn rejects_unrepresentable_edits_in_existing_integer_fields() {
    let mut graph = parse(&typed_fixture()).unwrap();
    let root = graph.root.clone();
    graph.objects.get_mut(&root).unwrap().properties["typed"]["42"] = "updated".into();
    graph.objects.get_mut(&root).unwrap().properties["long"] = json!(1.5);
    assert!(encode(&graph).is_err());
}

#[test]
fn new_interaction_maps_write_object_reference_keys() {
    let mut graph = parse(&fixture()).unwrap();
    let id = "11111111222233334444555555555555";
    let event = "99999999222233334444555555555555";
    graph.objects.insert(
        id.into(),
        Node {
            kind: "Axure:InteractionTreeMap".into(),
            properties: json!({"node-table":{event:{"$ref":event,"$token":3}}}),
        },
    );
    graph
        .set_dictionary_key_token(id, "/node-table", 3)
        .unwrap();
    let reopened = parse(&encode(&graph).unwrap()).unwrap();
    let ValueLayout::Dictionary { entries, .. } =
        &reopened.encoding.as_ref().unwrap().records[id].value
    else {
        panic!("dictionary");
    };
    let ValueLayout::Dictionary { entries, .. } = &entries[0].value_layout else {
        panic!("dictionary");
    };
    assert!(matches!(
        entries[0].key_layout,
        ValueLayout::Scalar { token: 3, .. }
    ));
    assert_eq!(
        reopened.objects[id].properties["node-table"][event]["$ref"],
        event
    );
}

#[test]
fn copied_templates_preserve_nan_widths_and_rebase_foreign_table_indices() {
    let source = parse(&typed_fixture()).unwrap();
    let mut target = parse(&fixture()).unwrap();
    let new_id = "11111111222233334444555555555555";
    target
        .copy_node_from(&source, &source.root, new_id)
        .unwrap();
    target.objects.get_mut(new_id).unwrap().properties["f32"] = json!(2.75);
    let encoded = encode(&target).unwrap();
    let reopened = parse(&encoded).unwrap();
    assert_eq!(
        reopened.objects[new_id].properties["typed"]["42"],
        "enum value"
    );
    assert_eq!(reopened.objects[new_id].properties["f32"], json!(2.75));
    assert_eq!(
        reopened.objects[new_id].properties["ref"]["$ref"],
        source.root
    );
    assert!(
        encoded
            .windows(8)
            .any(|w| w == 0x7ff8_0000_0000_0051_u64.to_le_bytes())
    );
    let ValueLayout::Dictionary { entries, .. } =
        &reopened.encoding.as_ref().unwrap().records[new_id].value
    else {
        panic!("dictionary");
    };
    let f32_entry = entries
        .iter()
        .find(|entry| entry.json_key == "f32")
        .unwrap();
    assert!(
        matches!(f32_entry.value_layout, ValueLayout::Scalar { token, .. } if token == (7 | 0x40))
    );
    assert!(encode(&reopened).unwrap() == encoded);
    // Creating a new native graph can also retain a foreign template's hints.
    let root = "aaaaaaaa222233334444555555555555";
    let mut fresh = Graph::new([11, 0, 0, 0], root.into(), root.into(), BTreeMap::new());
    fresh.copy_node_from(&source, &source.root, root).unwrap();
    let reopened = parse(&encode(&fresh).unwrap()).unwrap();
    assert_eq!(reopened.root, root);
    assert_eq!(
        reopened.root().unwrap().properties["long"],
        json!(i64::MAX - 11)
    );
}

#[test]
#[ignore = "Reads local RP 11 shipped samples; run explicitly for native format validation"]
fn every_installed_axure_package_roundtrips_byte_exactly() {
    use super::super::container;
    let root = std::env::var_os("RPSHOP_AXURE_SAMPLE_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(r"D:\Program Files (x86)\Axure\Axure RP 11\DefaultSettings")
        });
    assert!(
        root.is_dir(),
        "Native sample root missing: {}",
        root.display()
    );
    fn offsets(node: &Value, result: &mut Vec<u64>) {
        if let Some(parts) = node.get("parts").and_then(Value::as_object) {
            for (name, offset) in parts {
                if matches!(name.as_str(), "package" | "DesignDocument") {
                    result.push(offset.as_u64().unwrap());
                }
            }
        }
        if let Some(children) = node.get("children").and_then(Value::as_object) {
            for child in children.values() {
                offsets(child, result);
            }
        }
    }
    let mut files = 0;
    let mut packages = 0;
    let mut objects = 0;
    for directory in ["Training", "Libraries"] {
        for entry in std::fs::read_dir(root.join(directory)).unwrap() {
            let path = entry.unwrap().path();
            if !matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("rp" | "rplib")
            ) {
                continue;
            }
            let native = container::open(&path).unwrap();
            let mut parts = Vec::new();
            offsets(&native.index["root"], &mut parts);
            for offset in &parts {
                let bytes = native.part(*offset).unwrap();
                let graph = parse(&bytes)
                    .unwrap_or_else(|error| panic!("{} part {offset}: {error:#}", path.display()));
                objects += graph.objects.len();
                let saved = encode(&graph)
                    .unwrap_or_else(|error| panic!("{} part {offset}: {error:#}", path.display()));
                assert!(
                    saved == bytes,
                    "{} part {offset}: native bytes changed",
                    path.display()
                );
                let reopened = parse(&saved).unwrap();
                assert_eq!(graph.root, reopened.root);
                assert_eq!(graph.objects.len(), reopened.objects.len());
                packages += 1;
            }
            files += 1;
            eprintln!(
                "{}: {} native packages roundtripped exactly",
                path.display(),
                parts.len()
            );
        }
    }
    assert!(files >= 9 && packages >= 1253 && objects >= 41374);
    eprintln!("Validated {files} files, {packages} packages, {objects} objects");
}
