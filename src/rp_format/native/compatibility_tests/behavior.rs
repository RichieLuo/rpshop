use super::*;

#[test]
fn native_group_changes_preserve_invisible_subtrees_and_registry_references() {
    let (mut parts, before, bindings, mut groups) = grouped_order_fixture();
    let id = |value: u64| format!("{value:032x}");
    let package = parts[&10].package.clone();
    let outer = json!({"$ref":id(150),"$token":3,"unknown-reference-setting":47});
    let inner = json!({"$ref":id(151),"$token":3});
    let graph = parts.get_mut(&10).unwrap();
    graph.objects.insert(
        id(150),
        Node {
            kind: "Axure:DiagramObject:Layer".into(),
            properties: json!({"DiagramObjects":[inner],"unknown-setting":19}),
        },
    );
    graph.objects.insert(
        id(151),
        Node {
            kind: "Axure:DiagramObject:Layer".into(),
            properties: json!({"DiagramObjects":[],"unknown-setting":23}),
        },
    );
    let registry = graph.objects.get_mut(&id(102)).unwrap().properties["diagram-objects"]
        .as_array_mut()
        .unwrap();
    registry.insert(1, outer.clone());
    registry.push(inner.clone());
    for (group, object) in [(400, 150), (401, 151)] {
        groups.insert(
            group,
            GroupBinding {
                object: format!("{package}/{}", id(object)),
            },
        );
    }
    let untouched = [
        graph.node(&id(150)).unwrap().properties.clone(),
        graph.node(&id(151)).unwrap().properties.clone(),
    ];
    let mut after = before.clone();
    for widget in &mut after.widgets {
        if matches!(widget.id, 1..=3) {
            widget.groups.insert(0, 301);
        }
    }
    patch_native_groups(
        &mut parts,
        &package,
        &after,
        &before,
        &bindings,
        &BTreeMap::new(),
        &groups,
    )
    .unwrap();
    patch_native_order(
        &mut parts,
        &package,
        &after,
        &before,
        &bindings,
        &BTreeMap::new(),
    )
    .unwrap();
    let graph = &parts[&10];
    let registry = array(&graph.node(&id(102)).unwrap().properties["diagram-objects"]);
    let direct = direct_diagram_references(graph, registry).unwrap();
    assert_eq!(
        direct[0], outer,
        "opaque sibling stays outside the newly wrapped group"
    );
    assert_eq!(
        registry
            .iter()
            .find(|reference| reference_id(reference) == Some(id(151).as_str())),
        Some(&inner)
    );
    for (index, object) in [150, 151].into_iter().enumerate() {
        assert_eq!(
            graph.node(&id(object)).unwrap().properties,
            untouched[index]
        );
    }
    assert_eq!(layer_registry_members(graph, registry).unwrap().len(), 5);
}
