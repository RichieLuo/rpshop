use super::*;
#[test]
fn every_preset_roundtrips_and_groups_remain_individually_editable() {
    let mut p = Project::blank();
    for item in ITEMS {
        let widgets = item.create(&mut p, 10.0, 20.0);
        assert!(!widgets.is_empty());
        for w in &widgets {
            if let Some(points) = &w.vector_points {
                crate::vector_edit::validate_points(points).unwrap();
                assert!(crate::shape_geometry::svg_path(w).is_some());
                for (contour, _) in crate::shape_geometry::contours(w) {
                    if contour.len() < 3 {
                        continue;
                    }
                    let area: f32 = contour
                        .iter()
                        .zip(contour.iter().cycle().skip(1))
                        .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
                        .sum();
                    if area.abs() > 0.00001 {
                        assert!(
                            !crate::shape_geometry::triangles(&contour).is_empty(),
                            "canvas fill failed: {}",
                            item.id
                        );
                    }
                }
            }
        }
        p.pages[0].widgets.extend(widgets);
    }
    p.validate().unwrap();
    let json = serde_json::to_string(&p).unwrap();
    let decoded: Project = serde_json::from_str(&json).unwrap();
    for (before, after) in p.pages[0].widgets.iter().zip(&decoded.pages[0].widgets) {
        assert!(before == after, "preset roundtrip changed: {}", before.name);
    }
    let table = ITEMS
        .iter()
        .find(|i| i.id == "table")
        .unwrap()
        .create(&mut p, 0.0, 0.0);
    assert_eq!(table.len(), 12);
    assert!(table.iter().all(|w| w.groups == table[0].groups));
}
#[test]
fn search_matches_aliases_and_multiple_words() {
    assert!(
        ITEMS
            .iter()
            .find(|i| i.id == "textarea")
            .unwrap()
            .matches("多行")
    );
    assert!(
        ITEMS
            .iter()
            .find(|i| i.id == "flow-start")
            .unwrap()
            .matches("flow start")
    );
    assert!(!ITEMS[0].matches("repeater"));
}
