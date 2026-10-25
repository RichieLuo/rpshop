use super::*;

pub(super) fn set_interaction(
    parts: &mut BTreeMap<u64, Graph>,
    offset: u64,
    id: &str,
    interaction: &Interaction,
    pages: &BTreeMap<u64, String>,
) -> Result<()> {
    ensure!(
        matches!(interaction.action, Action::None | Action::Navigate),
        "原生保存暂不支持新增显示隐藏或设置文本交互，可保存单击跳转页面"
    );
    let graph = parts.get_mut(&offset).context("原生交互部件不存在")?;
    let has_authored = graph
        .node(id)
        .and_then(|node| node.properties.get("InteractionMap"))
        .and_then(|value| graph.resolve(value))
        .and_then(|map| map.properties["node-table"].as_object())
        .is_some_and(|table| {
            table
                .values()
                .filter_map(reference_id)
                .any(super::native_events::is_authored_case)
        });
    ensure!(
        !has_authored,
        "此元件包含新建事件，请通过事件与动作列表修改；原有交互已保留"
    );
    if interaction.action == Action::None {
        graph
            .objects
            .get_mut(id)
            .context("原生交互元件不存在")?
            .properties
            .as_object_mut()
            .unwrap()
            .remove("InteractionMap");
        return Ok(());
    }
    let target = interaction
        .target_page
        .and_then(|p| pages.get(&p))
        .context("原生跳转目标页面无效")?;
    let map = guid();
    let tree = guid();
    let event_node = guid();
    let case_node = guid();
    let action_node = guid();
    let event = guid();
    let case = guid();
    let action = guid();
    let target_id = guid();
    let literal = guid();
    let new_nodes = [
        (
            map.clone(),
            "Axure:InteractionTreeMap",
            json!({"node-table":{
            event_node.clone():{"$ref":event},case_node.clone():{"$ref":case},action_node.clone():{"$ref":action}},"tree":{"$ref":tree},"PackageMode":false}),
        ),
        (
            tree.clone(),
            "Axure:Tree",
            json!({"children":[{"$ref":event_node}],"tree":{"$ref":tree}}),
        ),
        (
            event_node.clone(),
            "Axure:Node",
            json!({"children":[{"$ref":case_node}],"parent-node":{"$ref":tree},"tree":{"$ref":tree}}),
        ),
        (
            case_node.clone(),
            "Axure:Node",
            json!({"children":[{"$ref":action_node}],"parent-node":{"$ref":event_node},"tree":{"$ref":tree}}),
        ),
        (
            action_node.clone(),
            "Axure:Node",
            json!({"children":[],"parent-node":{"$ref":case_node},"tree":{"$ref":tree}}),
        ),
        (
            event.clone(),
            "Axure:InteractionEvent",
            json!({"Disabled":false,"event-type":16,"master-containing-raised-event":{"$ref":EMPTY_ID,"$type":null,"$token":16},"raised-event-id":{"$guid":EMPTY_ID},"variable-id":{"$guid":EMPTY_ID}}),
        ),
        (
            case.clone(),
            "Axure:InteractionCase",
            json!({"Disabled":false,"Description":"","Condition":{"$ref":EMPTY_ID},"IsNewIfGroup":false}),
        ),
        (
            action.clone(),
            "Axure:InteractionLinkWindowAction",
            json!({"Disabled":false,"target":{"$ref":target_id},"link-type":1}),
        ),
        (
            target_id.clone(),
            "Axure:InteractionTarget",
            json!({"target-type":1,"target-page-id":{"$ref":target,"$type":"Axure:Page","$token":19},"target-literal":{"$ref":literal}}),
        ),
        (
            literal.clone(),
            "Axure:Expr:StringLiteral",
            json!({"value":"","variable-identifiers":[],"repeater-scope":{"$ref":EMPTY_ID},"TargetRepeaterScope":{"$ref":EMPTY_ID}}),
        ),
    ];
    for (id, kind, properties) in new_nodes {
        graph.objects.insert(
            id,
            Node {
                kind: kind.into(),
                properties,
            },
        );
    }
    graph.set_dictionary_key_token(&map, "/node-table", 3)?;
    graph
        .objects
        .get_mut(id)
        .context("原生交互元件不存在")?
        .properties["InteractionMap"] = json!({"$ref":map});
    Ok(())
}

pub(super) fn replace_reference(value: &mut Value, old: &str, new: &str) {
    match value {
        Value::Object(map) => {
            for key in ["$ref", "$guid"] {
                if map.get(key).and_then(Value::as_str) == Some(old) {
                    map.insert(key.into(), json!(new));
                }
            }
            for nested in map.values_mut() {
                replace_reference(nested, old, new);
            }
        }
        Value::Array(values) => {
            for nested in values {
                replace_reference(nested, old, new);
            }
        }
        _ => {}
    }
}
