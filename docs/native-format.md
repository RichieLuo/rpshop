# RP 11 file import: observed format

Investigation date: 2026-10-23. The source is a local RP 11 installation
(assembly version 11.0.0.4137). The investigation reads shipped sample files and static assembly
metadata; it does not launch the reference editor or alter its installation.

## Evidence and boundaries

REA `open_binary` rejected `DefaultSettings\Training\Quick Win.rp` because its
custom file format is unsupported. This is a format limitation, not evidence
that the file is encrypted or damaged.

REA `inspect_managed_artifact` identified the main application assembly as an
x64 .NET 8 assembly containing CIL and ReadyToRun code. Its SHA-256 is
`1cc3801cb9ca79ef509b17be1deecc054419df8752bf0b52c808967e08ec9bd4`.
Retained evidence: `ev_29095b34e892e52e044a84d47ea72477b4c58653026cf38c734c9b08bf867da7`,
summary `ev_c190106decf7be150fd0f4061d743e3af1ce602630df875822651b768170884d`.
REA reports partial metadata because the file contains unusual Module/Assembly
row counts and a GUID reference outside its admitted heap. Member inspection
also reached REA's 64 MiB metadata representation limit; it did not supply a
decompilation. Evidence:
`ev_fc47ccc08d0ab564b5bd50613b2b75e26da05391040c3af04afe9f7eda588291`.

The container facts below are verified directly against local file bytes. The
object token names and payload types were corroborated using static ILSpy
inspection of `RomX.AomC.ObjectStreamIds` and its reader/writer methods. The
application importer is an independent Rust implementation of the data format.
Extracted proprietary samples and decompilation output stay under ignored
`target/format-investigation`; they are not application resources.

## Outer `.rp` / `.rplib` container

All nine shipped samples have the same structure. They are not ZIP files.

| Offset | Encoding | Observed meaning |
| --- | --- | --- |
| 0 | `AC EF` | File signature |
| 2 | little-endian u16 | Outer format version, 11 in every sample |
| 4 | little-endian u32 | Length in bytes of the compressed directory record |
| 8 | bytes | Directory record |
| 8 + record length | bytes | Start of part records; directory offsets are relative to here |

The directory record starts with flag byte `1`, followed by two unsigned
7-bit continuation integers: decompressed size, compressed size. The following
compressed-size bytes are a raw LZ4 block. This reconstructs a UTF-8 JSON
directory with `root`, `children`, and `parts` keys. Child keys are 32-character
GUIDs. A part maps its name to a relative byte offset. Common parts include
`package`, `thumbnail`, `DesignDocument`, `DocumentSettings`, and
`11.0.0.4000.version`.

Each part record is `[little-endian u32 tag][little-endian u32 size][size bytes]`.
Observed tags are zero. Package and metadata content normally starts with a
GZIP signature (`1F 8B`); thumbnails contain raw JPEG. The directory's offsets
and sizes are checked independently before decompressing.

Example: `Quick Win.rp` begins
`AC EF 0B 00 2B 02 00 00 01 D2 05 A6 04 F0 03 ...`.
The directory record length is 555; its LZ4 block has 550 compressed bytes and
722 decompressed bytes. The first part starts at absolute offset 563. It has
tag 0, size 603, and GZIP data; the next part is at relative offset 611,
exactly `8 + 603` bytes later.

## Binary object package

After optional GZIP decompression, a normal object package has these fields:

1. i32 token 27 (`FileStart`).
2. Four i32 version components.
3. i32 data version 50001 (`DataVersion6_0_1`).
4. i32 token 31 (`StringList`), i32 count, then count strings, each encoded as
   i32 UTF-8 byte length followed by exactly that many bytes.
5. i32 token 30 (`GuidList`), i32 count, then count 16-byte .NET GUID values.
6. i32 token 26 (`PackageContext`), token 15 (`RootObjectId`) plus GUID table
   index, then token 18 (`RootObjectPackageId`) plus GUID table index.
7. Repeated token 9 (`ObjectRoot`), GUID table index, string table index for
   object type, and one dictionary value containing the object's properties.
8. i32 token 28 (`FileEnd`).

Both tables use one-based references. String index 0 means null; GUID index 0
means the empty GUID. In .NET's 16-byte representation, the first 4-byte field
and next two 2-byte fields are little endian; the final eight bytes retain
their order.

Value tokens are i32, with the low five bits selecting the value type:

| Token | Name | Payload |
| --- | --- | --- |
| 1 | KeyedDictionary | i32 pair count; recursively encoded key/value pairs |
| 2 | OrderedList | i32 count; recursively encoded values |
| 3 | InternalObjectReference | i32 GUID index |
| 4 | BinaryData | i32 length; raw bytes |
| 5 | ScalarInteger | i32 |
| 6 | ScalarDouble | IEEE f64 |
| 7 | ScalarSingle | IEEE f32 |
| 8 | ScalarString | i32 string index |
| 16 | PackageReference | i32 type string index; i32 GUID index |
| 17 | ScalarGuid | i32 GUID index |
| 19 | PackageId | i32 type string index; i32 GUID index |
| 20 | WeakObjectReference | i32 GUID index |
| 21 | ExternalObjectReference | i32 GUID index |
| 22 | ObjectId | i32 GUID index |
| 25 | ScalarBoolean | One byte |
| 29 | ScalarLong | i64 |

Integers and floating point values are little endian. Legacy text tokens 23/24
and pre-6.0.1 data versions are known token names but are not implemented in
the Rust parser. They did not occur in the tested sample packages. Unknown
tokens produce an error, including their byte offset.

`binary.rs` exposes a graph of typed objects, preserving references, scalars,
maps, lists, and binary data. Nonfinite floats used as absent Bézier handles
become JSON null in the editable view while their original bit patterns remain
in private encoding metadata. The parser checks lengths, table indices, duplicate object
IDs, nesting depth, item counts, root identity, and final byte consumption.

## Native object writing

`binary::encode(&Graph)` writes the same native object format. Parsed graphs
retain original tables, object order, dictionary order, typed dictionary keys,
full token flags, scalar widths, and nonfinite floating point payloads. Existing
unchanged scalar encodings are copied verbatim. Edited strings and new GUIDs
append table entries; editing a number retains its original i32/i64/f32/f64
representation and rejects values outside that representation's range.
This avoids treating a successful import as permission to discard unknown data.

`Graph::set_dictionary_key_token` supplies the native key domain when adding
entries to an originally empty dictionary, such as object reference keys in an
interaction node table. `Graph::copy_node_from` copies a widget or style template
between package graphs and rebases its encoding metadata into the destination
tables, preserving nonfinite control point handles and specialized numeric
types. References inside a copied object retain their original GUIDs until the
caller redirects them.

The Rust native encoder was exercised against all 1,253 packages and 41,374
objects in the nine installed `.rp`/`.rplib` samples. For every package,
`encode(parse(original))` equaled the complete original decompressed bytes.
Focused tests also change Unicode text and numeric fields, add object-reference
dictionary keys, and copy templates between different package tables. They
check the resulting values and retained scalar types after reopening. These
are native format checks; the original reference application was not executed to
validate edited files through its UI.

## Useful object relationships

Object names in this section omit their namespace prefix.

`DesignDocument` supplies document data and the sitemap. A page package's
root is `Page`; `diagram` references `Diagram`, whose
`diagram-objects` list gives drawing order. Widgets reference `MyStyle`, a
`Style` whose `PropList` entries contain numeric `DOAName` and `Value`.
Page-level `StyleCache` holds reusable style defaults. Widget text and labels
may use `RichTextBlock` or `name-block` structures. Images are stored as binary
data with references from styles and the document image map.

`PanelState` packages appear as children of page packages in the outer
directory. They have their own diagrams and widgets. These relationships must
be resolved during conversion; treating every package as a page would expose
configuration, style, and panel objects as spurious user pages.

Arbitrary vector widgets use normalized `ControlPoints` contours. Each point
contains `Path`, incoming `LowerHandle`, outgoing `HigherHandle`, and
`HigherSegmentExists`. Null/nonfinite handles use their corresponding anchor.
The optional Rust rasterizer preserves these curves, transparency, and holes
as PNG images with bounded antialiasing. It uses even-odd contour filling and
round stroke joins; those are importer rendering choices. Original path
editability, specialized fill rules, gradient paints, and all reference-editor rendering
details are not guaranteed by this conversion.

## Sample coverage

The independent Python schema probe fully consumed all 1,253 object packages
and 41,374 objects in the installed samples, with no unknown tokens or trailing
bytes:

| Sample | Packages | Objects |
| --- | ---: | ---: |
| Prototype Starter.rp | 36 | 1,004 |
| Prototyping Basics.rp | 11 | 703 |
| Quick Win.rp | 10 | 276 |
| UX Prototyping.rp | 24 | 828 |
| Default.rplib | 46 | 867 |
| Flow.rplib | 42 | 619 |
| Icons.rplib | 925 | 12,035 |
| Sample form patterns.rplib | 111 | 21,692 |
| Sample UI patterns.rplib | 48 | 3,350 |

Some nested object packages report source versions 9 or 10 while using the
observed version-11 outer container and data version 50001. This does not
establish support for original RP 9/10 outer files, every RP 11 patch release,
encrypted or future formats, or every widget and interaction. Parsing a graph
successfully and reproducing its application behavior are separate checks.
The importer must report any content conversion limits explicitly.
