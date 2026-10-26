# RPShop branding

`rpshop-logo.png` is the RPShop application artwork: an original geometric owl avatar with a four-node prototype motif, on a pale lavender rounded tile. Its layered animal illustration shares the SQLShop brand family's crisp vector style while keeping RPShop's own purple and mint palette and a distinct mascot. `rpshop-icon.png` is a simplified, small-size version used in the editor header.

`rpshop-logo.svg` is an editable vector companion. The Windows build script converts the PNG into a multi-resolution ICO (16, 24, 32, 48, 64, 128 and 256 pixels) in Cargo's build output directory and embeds it into `rpshop.exe`.

Palette: purple `#6129E8`, deep purple `#5422CF`, lavender `#AD8DF0`, pale plate `#F0EAFD`, mint `#A3F5BF`.

Windows uses a single SQLShop-style title bar and the normalized application icon for its brand mark. The source images remain unchanged; `src/branding_icon.rs` trims faint export noise and generates the same 80% rounded plate and transparent margins as the other Shop applications. Runtime and embedded EXE icons share this renderer. See [the shared branding conventions](../../docs/branding.md).

`rpshop-owl-logo.png` is the wide owl-and-wordmark banner used at the top of the README, centered at 560 pixels to match the other Shop projects. Application icons continue to be generated from `rpshop-logo.png`; `rpshop-icon.png` retains its compact artwork role in the existing macOS/Linux editor header.
