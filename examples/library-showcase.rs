//! Regenerate the independently authored built-in library gallery.
//! cargo run --example library-showcase -- [output-directory]
use anyhow::Result;
use rpshop::model::{Page, Project, TextLayout, Widget, WidgetKind};
use rpshop::widget_library::{Category, ITEMS};

fn main() -> Result<()> {
    let directory = std::env::args().nth(1).unwrap_or_else(|| "examples".into());
    let directory = std::path::Path::new(&directory);
    std::fs::create_dir_all(directory)?;
    let mut project = Project::blank();
    project.name = "RPShop 组件画廊".into();
    project.pages.clear();
    for category in Category::ALL {
        let items: Vec<_> = ITEMS
            .iter()
            .filter(|item| item.category == category)
            .collect();
        if items.is_empty() {
            continue;
        }
        let mut page = Page {
            id: project.allocate_id(),
            name: category.label().into(),
            width: 1280.0,
            height: (items.len().div_ceil(3) as f32 * 330.0 + 140.0).max(950.0),
            canvas: Default::default(),
            background: "#FFFFFF".into(),
            widgets: Vec::new(),
        };
        let mut heading = Widget::new(project.allocate_id(), WidgetKind::Text, 40.0, 28.0);
        heading.text = format!("{} · {} 项", category.label(), items.len());
        heading.width = 1100.0;
        heading.height = 56.0;
        heading.font_size = 30.0;
        heading.text_layout = Some(TextLayout {
            horizontal: 0,
            vertical: 1,
            padding: [0.0; 4],
            wrap: false,
            bold: true,
        });
        page.widgets.push(heading);
        for (index, item) in items.iter().enumerate() {
            let x = 40.0 + (index % 3) as f32 * 410.0;
            let y = 118.0 + (index / 3) as f32 * 330.0;
            let mut caption = Widget::new(project.allocate_id(), WidgetKind::Text, x, y);
            caption.text = format!("{:02}  {}", index + 1, item.label);
            caption.width = 370.0;
            caption.height = 28.0;
            caption.font_size = 14.0;
            caption.text_color = "#858A99".into();
            page.widgets.push(caption);
            page.widgets.extend(item.create(&mut project, x, y + 46.0));
        }
        project.pages.push(page);
    }
    project.save(&directory.join("library.rpshop"))?;
    std::fs::write(
        directory.join("library.html"),
        rpshop::export::html(&project)?,
    )?;
    for (index, page) in project.pages.iter().enumerate() {
        let name = if index == 0 {
            "library.svg".into()
        } else {
            format!("library-{}.svg", index + 1)
        };
        std::fs::write(directory.join(name), rpshop::export::svg(page)?)?;
    }
    println!(
        "已生成 {} 个分类、{} 项组件的画廊：{}",
        project.pages.len(),
        ITEMS.len(),
        directory.display()
    );
    Ok(())
}
