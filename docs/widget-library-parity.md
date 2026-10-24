# 组件库与 Axure RP 11 对照

调查日期：2026-10-24。目标是本机 `D:\Program Files (x86)\Axure\Axure RP 11` 安装副本。按原厂库的实际目录和组件类型核对 RPShop；所有新增预设、图标和渲染实现均由 RPShop 独立编写。

## 证据与范围

本轮通过 REA 静态分析已安装的播放器文件 `resources/scripts/axure/axQuery.std.js`。分析父证据为 `ev_9f92fd9a43d6dabe660687186b528db78cc28b412e6445bbd9a5b49298b00ff6`，摘要为 `ev_265bd305e5c35459697e96531c8b06daa37c14f508b96fc545be136e58d1ee32`。目录 SHA-256 为 `bb3938daef4dcfec7bf4fe7821673c987367f8b7b8fd4c9df4919e8c9da42e8f`。选择的文件成功解析，另外 48 个相关文件未进入这次分析；关系与语义覆盖为 partial，不能据此宣称整个播放器已经复原。

直接阅读该文件第 6–34 行可观察到 table、menuObject、repeater、dynamicPanel、textBox、textArea、listBox、comboBox、checkbox、radioButton、imageMapRegion、treeNodeObject、inlineFrame 等不同类型声明。这些是播放器的静态类型声明；没有执行 Axure 桌面应用或验证其全部交互。声明本身也不等于默认库中有一项一一对应的预设。

`.rplib` 清单另由 RPShop 的只读容器/对象图解析器读取，**不属于 REA 反编译结果**。检查脚本为 [inspect-library.rs](../scripts/inspect-library.rs)，读取 `DesignDocument` 的 `PackageInfo` 页面、sitemap 分类，以及对应对象包中的 `Axure:DiagramObject:*` 类型。没有复制库中的图形、图片、样式或交互实现。完整目录在 [library-inventory.json](library-inventory.json)，图标库元数据在 [library-icons-inventory.json](library-icons-inventory.json)。

主程序集的既有观察仍有效：版本 `11.0.0.4137`，SHA-256 `1cc3801cb9ca79ef509b17be1deecc054419df8752bf0b52c808967e08ec9bd4`。REA managed members 检查受到 64 MiB 表示预算限制，不能给出完整桌面编辑器成员或行为；本轮没有把这个失败解释为组件不存在。

| 原厂库 | 实际页面条目数 | 范围说明 |
| --- | ---: | --- |
| Default | 40 | Common 16、Forms 6、Menus and table 4、Markup 8、Dynamic 6 |
| Flow | 37 | 流程图形、图片与 Snapshot |
| Icons | 920 | 仅名称、分类和 ID 清单；未提取图形 |
| Sample form patterns | 48 | 包括 4 项 Instructions；其余也是复合示例，不等于基础控件类型数 |
| Sample UI patterns | 15 | 包括 3 项 Instructions；含轮播、选项卡、分页、弹窗、可筛选表格等复合示例 |

这些数字是该安装副本中的库页面数，不是所有 Axure 版本的全量能力数字。默认库与流程图库之间有重名项，不能简单相加后当作独立基础类型数。

## Default 的 40 项逐项对照

下表把本轮实现方向与仍需补齐的语义分开。视觉预设复用 RPShop 的基础类型；单选、文本域、下拉和列表使用各自的真实控件类型。组合预设可解组编辑，不具备原厂专用对象的数据模型。最终可运行范围以对应源码和验证结果为准。

| 分类 | Axure 库项 | RPShop 对应与边界 |
| --- | --- | --- |
| Common | Box 1 | 矩形；可编辑填充、边框、文字 |
| Common | Box 2 | 矩形/圆角矩形样式预设；不要求复制原厂样式值 |
| Common | Box 3 | 矩形样式预设；不要求复制原厂样式值 |
| Common | Image | 图片组件，读取用户选取的本地图片 |
| Common | Placeholder | 带对角线的矢量占位符 |
| Common | Button | 次要按钮样式预设，可添加点击动作 |
| Common | Primary button | 主要按钮样式预设，可添加点击动作 |
| Common | Link button | 文本链接样式；用已有交互设置工程内页面跳转，尚非完整超链接富文本模型 |
| Common | Heading 1 | 一级标题预设 |
| Common | Heading 2 | 二级标题预设 |
| Common | Heading 3 | 三级标题预设 |
| Common | Label | 单行文本预设 |
| Common | Paragraph | 可换行段落预设 |
| Common | Horizontal line | 水平直线 |
| Common | Vertical line | 垂直直线预设/旋转线段；沿用同一线段几何 |
| Common | Ellipse | 椭圆 |
| Forms | Text field | 单行输入框 |
| Forms | Text area | 真实多行文本域；与单行输入区分 |
| Forms | Droplist | 真实下拉选择；选项逐行编辑 |
| Forms | List box | 真实列表选择；选项逐行编辑 |
| Forms | Checkbox | 真实复选框；预览可勾选 |
| Forms | Radio button | 真实单选控件；需要明确互斥组语义 |
| Menus and table | Classic table | 可解组的表格单元格组合；没有专用插入行列、合并单元格模型 |
| Menus and table | Classic menu - horizontal | 横向菜单组合；子菜单展开和键盘导航未对齐 |
| Menus and table | Classic menu - vertical | 纵向菜单组合；子菜单展开和键盘导航未对齐 |
| Menus and table | Tree | 文本节点组合；尚非可折叠、可编辑层级的数据控件 |
| Markup | Sticky 1 | 便签样式预设方向；是否已提供独立入口以最终目录为准 |
| Markup | Sticky 2 | 便签样式预设方向；是否已提供独立入口以最终目录为准 |
| Markup | Sticky 3 | 便签样式预设方向；是否已提供独立入口以最终目录为准 |
| Markup | Sticky 4 | 便签样式预设方向；是否已提供独立入口以最终目录为准 |
| Markup | Circle marker | 圆形编号标记预设方向；可用椭圆和文字组成 |
| Markup | Drop marker | 水滴编号标记尚需独立预设 |
| Markup | Horizontal arrow | 矢量箭头；可缩放、旋转和导出 |
| Markup | Vertical arrow | 垂直箭头预设/旋转矢量箭头；沿用同一路径 |
| Dynamic | Dynamic panel | 现有基础容器不等价；原生导入预览保留多状态，新建与编辑状态仍缺失 |
| Dynamic | Hot spot | 透明点击区域预设；沿用已有点击事件，不等于完整 ImageMapRegion 编辑器 |
| Dynamic | Inline frame | 已导入原生框架的预览有部分支持；新建、框架属性编辑尚缺 |
| Dynamic | Repeater - card | 尚无新建数据驱动中继器、数据集与行事件编辑 |
| Dynamic | Repeater - table | 尚无新建数据驱动中继器、排序/筛选/分页编辑 |
| Dynamic | Snapshot | 尚无绑定其他页面/组件的快照刷新模型；普通图片不应冒充 |

## Flow 的 37 项

实际名称为：Actor、Beveled rectangle、Bracket、Collate、Database、Delay、Diamond、Display、Document、Ellipse、File、Hexagon、Image、Internal storage (RAM)、Keying、Logical or、Magnetic tape、Manual input、Multi-document、Off page connector、Offline storage、Parallelogram、Punched card、Punched tape、Rectangle、Rounded rectangle、Semicircle、Snapshot、Sort、Stacked file、Stacked rectangle、Stacked rounded、Stored data、Subroutine/Predefined process、Summing junction、Trapezoid、Triangle。

本轮以独立矢量路径补常用流程、判断、开始/结束、数据、文档、数据库、子流程、手动输入、准备和连接点；其中矩形、菱形、圆角、平行四边形、数据库、文档、六边形等与 Flow 项目的主要视觉用途对应。Actor、Bracket、多页文档、磁带、打孔带等其他图形，以及能吸附并跟随对象的连接器仍需补齐。绘制一个箭头形状不代表具备连接器行为。

## 后续对齐重点

1. 目录与查找：用分类、搜索和稳定的预设 ID 承载组件，不把每种标题或颜色都扩成底层类型；保留窄侧栏、拖入和独立滚动。
2. 编辑与保存：每种真正新控件必须贯通默认值、画布、属性、复制粘贴、撤销、独立工程、HTML/SVG 和原生文件读写。不能只增加组件库按钮。
3. 真实语义：表格、树、菜单从可编辑组合升级为专用模型；动态面板、中继器、框架和快照按各自的数据/状态模型实现。
4. 图标与扩展库：920 项原厂图标清单只用于了解范围；当前原创常用图标集不宣称与其数量、形状或许可相同。用户库导入、实例复用和外部库管理仍是单独能力。

文件身份：Default SHA-256 `9b4f398eab76f6e081f3275824ad15396f670fd2448e7e36b4e4399c40f85d50`；Flow `95d85ffaddac36144ae8ebd645341b2e557f0a6157b092319e7dac7b71d9d451`；Icons `83f450088f81f6047dfb196919fff3a4aae2f3adb56668da4d4c8cfd3b813bda`；Sample form patterns `1044b386c12c170876c0423ccd07b515d9c9e063d4cde108cb6a4bbe96131179`；Sample UI patterns `cf1787fded7a74278209f9aee4b07a266a10360f2e0dd552c49cca158f65e36b`。

REA 摘要与原始父证据 ID 保存在 [library-rea-evidence.json](library-rea-evidence.json)。本机完整静态图证据包保留在 `target/library-rea-full-evidence.json`，不把体积较大的分析中间产物作为组件实现或图形资产分发。
