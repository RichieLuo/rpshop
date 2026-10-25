use super::*;

/// Reorder only proven sibling lists. Groups, tables, states, and read-only
/// previews must remain contiguous blocks in their parent's flat projection.
/// Candidate changes are simulated and compared with the entire requested
/// page before any source list is changed.
pub(super) fn patch_native_order(
    parts: &mut BTreeMap<u64, Graph>,
    package: &str,
    page: &Page,
    before: &Page,
    bindings: &BTreeMap<u64, Binding>,
    preview_parents: &BTreeMap<u64, u64>,
) -> Result<()> {
    let projection = native_order_projection(parts, page, before, bindings, preview_parents)?;
    let orders = projected_native_order(parts, package, page, projection)?;
    for ((offset, id, field), references) in orders {
        parts
            .get_mut(&offset)
            .unwrap()
            .objects
            .get_mut(&id)
            .unwrap()
            .properties[&field] = Value::Array(references);
    }
    Ok(())
}

pub(super) fn native_order_projection<'a>(
    parts: &'a BTreeMap<u64, Graph>,
    page: &Page,
    before: &Page,
    bindings: &BTreeMap<u64, Binding>,
    preview_parents: &BTreeMap<u64, u64>,
) -> Result<NativeOrderProjection<'a>> {
    let mut previews: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for widget in &before.widgets {
        if let Some(parent) = preview_parents.get(&widget.id) {
            previews.entry(*parent).or_default().push(widget.id);
        }
    }
    let mut widgets = BTreeMap::new();
    let page_ids = page
        .widgets
        .iter()
        .chain(&before.widgets)
        .map(|widget| widget.id)
        .collect::<BTreeSet<_>>();
    for (id, binding) in bindings {
        if !page_ids.contains(id) {
            continue;
        }
        ensure!(
            widgets.insert(binding.object.clone(), *id).is_none(),
            "重复原生对象的层级映射不唯一，暂不支持重排"
        );
    }
    Ok(NativeOrderProjection {
        parts,
        widgets,
        packages: parts
            .iter()
            .map(|(offset, graph)| (graph.package.clone(), *offset))
            .collect(),
        previews,
        lists: std::cell::RefCell::new(BTreeSet::new()),
    })
}

pub(super) fn projected_native_order(
    parts: &BTreeMap<u64, Graph>,
    package: &str,
    page: &Page,
    projection: NativeOrderProjection<'_>,
) -> Result<NativeListOrders> {
    let page_offset = *projection
        .packages
        .get(package)
        .context("原生页面层级不存在")?;
    let page_graph = &parts[&page_offset];
    let page_diagram = page_graph
        .root()
        .and_then(|root| root.properties.get("diagram"))
        .and_then(reference_id)
        .context("原生页面画布不存在")?;
    projection.object(
        page_offset,
        page_diagram,
        &NativeListOrders::new(),
        &mut BTreeSet::new(),
        0,
    )?;
    let reachable = projection.lists.borrow().clone();
    let positions = page
        .widgets
        .iter()
        .enumerate()
        .map(|(index, widget)| (widget.id, index))
        .collect::<BTreeMap<_, _>>();
    let mut orders = NativeListOrders::new();
    for (offset, graph) in parts.iter() {
        for (id, node) in &graph.objects {
            let field = if node.kind.ends_with(":Layer") {
                "DiagramObjects"
            } else if matches!(node.kind.as_str(), "Axure:Diagram" | "Axure:PanelDiagram") {
                "diagram-objects"
            } else {
                continue;
            };
            let Some(references) = node.properties[field].as_array() else {
                continue;
            };
            if !reachable.contains(&(*offset, id.clone(), field.to_owned())) {
                continue;
            }
            let diagram = matches!(node.kind.as_str(), "Axure:Diagram" | "Axure:PanelDiagram");
            let canonical = if diagram {
                direct_diagram_references(graph, references)?
            } else {
                references.clone()
            };
            let mut projected = Vec::with_capacity(canonical.len());
            let mut complete = true;
            for reference in &canonical {
                let Some(child) = reference_id(reference) else {
                    complete = false;
                    break;
                };
                let ids = projection.object(
                    *offset,
                    child,
                    &NativeListOrders::new(),
                    &mut BTreeSet::new(),
                    0,
                )?;
                let child_positions = ids
                    .iter()
                    .filter_map(|id| positions.get(id).copied())
                    .collect::<Vec<_>>();
                if ids.is_empty() || child_positions.len() != ids.len() {
                    complete = false;
                    break;
                }
                let first = *child_positions.iter().min().unwrap();
                let last = *child_positions.iter().max().unwrap();
                ensure!(
                    last - first + 1 == ids.len(),
                    "此层级操作会跨越原生父容器或动态面板状态；仅支持同一直接父容器内重排"
                );
                projected.push((first, reference.clone()));
            }
            if complete {
                projected.sort_by_key(|(position, _)| *position);
                let reordered = projected
                    .into_iter()
                    .map(|(_, reference)| reference)
                    .collect::<Vec<_>>();
                if reordered != canonical {
                    let reordered = if diagram {
                        rebuild_diagram_registry(
                            graph,
                            references,
                            &layer_registry_members(graph, references)?,
                            &reordered,
                        )?
                    } else {
                        reordered
                    };
                    orders.insert((*offset, id.clone(), field.into()), reordered);
                }
            }
        }
    }
    let offset = *projection
        .packages
        .get(package)
        .context("原生页面层级不存在")?;
    let graph = &parts[&offset];
    let diagram = graph
        .root()
        .and_then(|root| root.properties.get("diagram"))
        .and_then(reference_id)
        .context("原生页面画布不存在")?;
    let actual = projection.object(offset, diagram, &orders, &mut BTreeSet::new(), 0)?;
    let desired = page
        .widgets
        .iter()
        .map(|widget| widget.id)
        .collect::<Vec<_>>();
    ensure!(
        actual == desired,
        "此层级操作无法保持原生父容器、表格或状态结构；仅支持同一直接父容器内可完整映射的元件重排"
    );
    Ok(orders)
}

pub(super) fn rebuild_diagram_registry(
    graph: &Graph,
    original: &[Value],
    members_before: &[Value],
    desired: &[Value],
) -> Result<Vec<Value>> {
    let old_members = members_before
        .iter()
        .filter_map(reference_id)
        .collect::<BTreeSet<_>>();
    // Preserve files that already use direct-only root lists. A diagram with
    // no existing groups uses the native registry convention for new groups.
    if !old_members.is_empty()
        && !original
            .iter()
            .any(|reference| reference_id(reference).is_some_and(|id| old_members.contains(id)))
    {
        return Ok(desired.to_vec());
    }
    let members = layer_registry_members(graph, desired)?;
    let new_members = members
        .iter()
        .filter_map(reference_id)
        .collect::<BTreeSet<_>>();
    let roots = desired
        .iter()
        .filter_map(reference_id)
        .collect::<BTreeSet<_>>();
    let originals = original
        .iter()
        .filter_map(|reference| reference_id(reference).map(|id| (id, reference)))
        .collect::<BTreeMap<_, _>>();
    let mut direct = desired.iter();
    let mut emitted = BTreeSet::new();
    let mut result = Vec::new();
    for reference in original {
        let id = reference_id(reference).context("原生注册表包含无法识别的引用")?;
        if old_members.contains(id) {
            if new_members.contains(id) && !roots.contains(id) {
                ensure!(emitted.insert(id.to_owned()), "原生注册表存在重复对象引用");
                result.push(reference.clone());
            }
        } else if let Some(reference) = direct.next() {
            let id = reference_id(reference).context("原生父层级引用无效")?;
            ensure!(
                emitted.insert(id.to_owned()),
                "原生注册表存在重复父对象引用"
            );
            result.push(reference.clone());
        }
    }
    for reference in direct {
        let id = reference_id(reference).context("原生父层级引用无效")?;
        ensure!(
            emitted.insert(id.to_owned()),
            "原生注册表存在重复父对象引用"
        );
        result.push(reference.clone());
    }
    for reference in &members {
        let id = reference_id(reference).context("原生组合成员引用无效")?;
        if emitted.insert(id.to_owned()) {
            result.push(originals.get(id).copied().unwrap_or(reference).clone());
        }
    }
    Ok(result)
}
