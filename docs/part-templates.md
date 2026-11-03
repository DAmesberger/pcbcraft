# Part templates as Solidcraft documents

Contract and design note, 2 November 2026. Design only: nothing here is built except the spike in `crates/solidcraft-integration/tests/template_spike.rs`. Base: Pcbcraft `component-model` at 55f47d6 and the Solidcraft fork `pcbcraft-via2` at d53b131 (which Pcbcraft pins). Rollback points are the tags `pre-part-templates` (Pcbcraft) and `pcbcraft-pre-part-templates` (fork).

A **part template** is a parametric `.solidcraft` design, made in Solidcraft with its materials, which also declares what Pcbcraft needs from it:

- an interface of named, typed parameters;
- anchors;
- lead contacts tagged with terminal keys;
- a land pattern;
- a courtyard and keep-outs;
- marking surfaces and marking slots;
- body roles and materials;
- validation rules.

A package refers to one exact template revision and gives it parameter values. Pcbcraft builds the 3D model, the land pattern, the envelope, the lead contacts and the marking surfaces from it. Complex parts (power modules, shielded connectors and inductors) and purely mechanical parts (standoffs, heatsinks, brackets) no longer need Rust code per family.

Vocabulary: "template" already names package-row templates, footprint templates and the template gallery ([glossary](../GLOSSARY.md)). This note always says **part template**. In code the names are `PartTemplate…`, and in commands `part_template`.

**Status (founder, 2 November 2026): paused.** The design and the spike are kept, and nothing further is built until the founder resumes the work. Push-and-shove and the ESP32 reference board come first. The founder decided:

- the open questions on keeping typed forms as views, metadata over a format change, derived pads, generic authoring panels and vendor STEP with a recorded licence go as recommended;
- release 1 varies instances by **render-time markings only**, with no per-instance geometry overrides (the `instance` binding of D5 is deferred);
- upstream PRs are approved to *prepare* (§8), starting with the **photoreal look** rather than the performance work. Each PR still needs the founder's OK before it is opened.

**Revision (founder, 3 November 2026): lean parts, Solidcraft stays generic.** No Pcbcraft-specific data goes into Solidcraft, in the fork or upstream: no metadata namespace, attribute panel or interface block (this drops SC2, SC5 and the `org.pcbcraft.*` interface below). The fork extends only the generic material model: a data-only material node graph (MaterialX/OpenPBR-style; clear coat is a graph input, not a format field), designed in the fork's docs/material-graph.md and proposed upstream once proven. A part is an ordinary Solidcraft design: named parameters choose the variant; its origin is the footprint origin and z = 0 its seating plane; marking areas and anchors are named construction planes and points that Pcbcraft finds by documented names and checks itself. Footprints stay Pcbcraft's (package forms and the datasheet assistant) and do not come from the part file. Sections below that assume the interface metadata are superseded where they conflict with this paragraph.

## 1. Decisions

| # | Decision | Why |
|---|---|---|
| D1 | A part template is an ordinary Solidcraft design. Pcbcraft's interface lives in namespaced document metadata (`org.pcbcraft.part_template.N`). It refers to Solidcraft objects only by stable names (parameter, body, feature and sketch-curve ids) inside reference objects that renames follow (§3 SC2). | Plain Solidcraft opens and edits templates without a format change, and upstream Solidcraft stays compatible. Solidcraft has no per-body attribute store; metadata with followed references is the smallest addition. |
| D2 | A template's history is **inserted** into the board design, one component definition per distinct parameter set. Its features are cloned with a typed remap, its parameters prefixed (`pkg<id>r<rev>_<name>`) and its bodies named `<definition> <template body>` (§3 SC1). Evaluated bodies are never inserted as STEP. | The saved `.solidcraft` board stays parametric, as AGENTS.md requires, and needs no second evaluation path. The spike shows the insert gives the same geometry exactly, at no measurable overhead (§2). |
| D3 | Pcbcraft keeps one definition per distinct `(template revision, parameter values)`, under the key `ParamSetKey = sha256(asset hash ‖ canonical values ‖ engine revision)`. Instances share it. | Solidcraft's evaluation cache keys on feature identity, so two definitions with equal values do not share results (measured). Deduplication is Pcbcraft's job. |
| D4 | Parameter values a package or instance gives are **literal quantities** (number and unit, count, boolean, choice, text), never expressions. | One definition cannot then refer to another's parameters, and values can be validated and hashed. |
| D5 | Each parameter declares its `binding`. `package` parameters are geometry fixed by the package. `instance` parameters are geometry an instance may override, each distinct set becoming another definition. `render` parameters are display only: markings and material slots, which never rebuild geometry. | This splits render-time variation (the ESP32 module's marking, a resistor's "101", an electrolytic sleeve's text and stripe) from geometry. |
| D6 | **The land pattern is derived**, not linked. Pcbcraft evaluates the template's tagged land sketch at the package's values and writes ordinary footprint pads (`MaskRule` and `PasteRule` from the tags), recording which template revision and `ParamSetKey` they came from. A footprint edited by hand is detached from it. | A footprint stays independent of the package body ([part library design](part-library-design.md)). Board rules and lint apply to real pads as they do today. Template-driven sketch sources (`FootprintSource`) are a later option (O3). |
| D7 | Markings stay display-only (F7). A template declares marking surfaces (planes and cylinders), a default `MarkingSpec` and named slots. `SurfaceRef::Anchor` resolves against the template's surfaces instead of the hard-coded `surface::anchors(form)`. Slots bind to `{mpn}`, `{field:…\|code}`, `{svg:NAME}`, `{param:NAME}`, `{designator}` and the rest. | The marking engine, codes, wraps, finishes, QR codes and decals stay as built (D4–D7). Only where surfaces and defaults come from changes. |
| D8 | Body looks bind to Pcbcraft `MaterialId`s or to named material slots, through the template metadata. The Solidcraft appearance the author assigns carries the id in its name (`pcbcraft:<material_id>`) and stores the opaque PBR projection. | The authored file shows the right looks in plain Solidcraft. Pcbcraft draws the full look (clear coat, pattern) from its cited table ([materials](materials.md)). Persisting coat and pattern in Solidcraft's `Look` is a later fork and upstream change (SC6). |
| D9 | A new revisioned library kind `PartTemplate` holds the template bytes (packed by hash) and the inspected interface, which publish and build check again. `PackageForm::PartTemplate` and `PackageModel::PartTemplate` refer to it. | Same pattern as the Model kind (D1 as built): exact revisions, snapshots, bundles and update plans. |
| D10 | Built-in families migrate to **built-in part templates**. The typed forms (`ChipForm`, `ModuleForm`, …) stay as typed views; a pure function maps each to the template's values. The Rust builders are deleted only once every starter row of the family passes parity (§7). | The form UI, package rows and footprint family templates keep working. Migrating is a per-family switch with a measured gate. |
| D11 | The fork's authoring interface is **generic** (exposed parameters, namespaced attributes on design objects with schema-driven forms, a material library). Pcbcraft's semantics come in as a JSON schema file and are checked by Pcbcraft (`pcbcraft-cli part-template check`). | Nothing Pcbcraft-specific lands in the shared engine, so the work can go upstream. |
| D12 | A template is untrusted input. Its size is bounded, its evaluation runs off the UI thread with cancel and a time budget, and built-in templates may contain no import features. | Same input policy as models and datasheets. |

## 2. Spike: inserting a template's history (measured)

**Question.** Can a `.solidcraft` design, given as bytes, be inserted into another design so that its feature history and parameters stay live, with per-definition overrides, and what does that cost? This is the hardest unknown. `component.insert_design` only brings in evaluated solids, through STEP, as an Import feature (`crates/engine/src/cmd/component_link.rs`). No per-occurrence parameter override exists, and occurrences of one definition always share geometry.

**Method.** `crates/solidcraft-integration/tests/template_spike.rs` is an ignored test that uses only the public APIs of the pinned fork.

1. **Stand-in templates.** The generated ESP32-S31-WROOM-3 model (the example project's module form) and a QFN-48 7 × 7 model are each saved as their own design, with their parameters renamed to plain names (`can_height`, `body_height`, …). These stand in for authored templates.
2. **Two insert paths.** (a) The session clipboard: `timeline.copy` and `timeline.paste`, after `Document::rename_param`. (b) A typed insert done on the document: each feature cloned with `Feature::remap_clone` over maps of feature ids, feature names, body names (from the template's own evaluated model) and parameter names (`Document::param_defs`); the parameters copied with `clone_expression`; one `Session::refresh`.
3. **What is compared.** Per body: bounding box and volume against the generated build, two parameter sets side by side, and a save and reopen.

Run it with `cargo test --release -p pcbcraft-solidcraft --test template_spike -- --ignored --nocapture`. Measured on an Apple M5 Max (macOS, release build, one thread), over three runs:

| Measure | WROOM-3 (110 bodies, 20 features) | QFN-48 (50 bodies, 13 features) |
|---|---|---|
| Today's generated build (`Native::new` + `build_definition`) | 103–133 ms | 0.93–1.20 s |
| Cold: read bytes + evaluate the template alone | 1–2 ms + 46–52 ms | 0.2 ms + 0.92–1.15 s |
| Cold from a container carrying its saved model (`Model::adopt`, default values) | 8.7 ms | 7.2 ms |
| Slowest feature | `can round` fillet, 33–38 ms | `frame pockets` Boolean cut, 0.86–0.93 s |
| Tessellate every body | 96 ms, 6 306 triangles | 23–28 ms, 3 328 triangles |
| Override one parameter on the template (incremental) | 39–45 ms | 0.93–1.07 s |
| Set it back (Solidcraft's evaluation cache) | 0.2–0.3 ms | 0.2 ms |
| **Typed insert into a board** (component, parameters, features, one evaluation) | **45–52 ms** | **0.93–1.02 s** |
| Typed insert, second parameter set beside the first | 47–52 ms, 220 bodies, no errors | 0.93–1.02 s, 100 bodies, no errors |
| Geometry against the generated build | bounding-box Δ 0, volume Δ 0, 110/110 bodies | bounding-box Δ 0, volume Δ 0, 50/50 bodies |
| Change an inserted parameter in the board | 38–48 ms | 1.0–1.3 s |
| Second definition set to the first one's values | 38–41 ms (no result shared) | 1.0 s (no result shared) |
| …and back to its own values (evaluation cache) | 0.4 ms | 0.3 ms |
| Board with two definitions, saved then reopened and evaluated | 91–101 ms, no errors, 235 KiB JSON | 1.9–2.0 s, no errors, 27 KiB |
| 100 more placements of one definition (`occurrence.copy` with `translations`) | 0.1 ms | 0.1 ms |

**Findings.**

1. **Feasible without a format change.** A typed insert keeps every feature and parameter live, reproduces the geometry exactly, and a reopened board evaluates with no errors. Its cost is the template's own evaluation; the insert itself adds nothing measurable. The WROOM-3 template costs about 50 ms cold and 0.3 ms from cache.
2. **The clipboard path is not usable.** `timeline.paste` assumes the clipboard came from the same session: it reads each copied feature's body names from the host's own results (`s.model.result(f.id)`). Across documents it renames bodies as copies (`pkgT board (2)`). A second insert then references the wrong bodies (`unknown body pkgT row 1 pad`, six failing features). The fork needs a dedicated insert with explicit, deterministic name maps (SC1); the spike's `insert_history` is its reference implementation.
3. **Equal parameter sets do not share.** Solidcraft's evaluation cache keys on feature identity, so equal values in two definitions are evaluated twice. Hence D3.
4. **Cost is in the authoring.** One Boolean cut makes a QFN-48 take about 1 s, in today's builder and in a template alike. The saved-model cache makes a template's default set cold-load in under 10 ms. Templates should ship with their saved model. Authoring guidance and an advisory lint (`part_template.slow`: cold evaluation above 100 ms, a target) push authors towards disjoint bodies over many-tool cuts, as the builders already do ([build cost](pcb-realism.md#build-cost-measured)).

**Not covered by the spike**, and so carried as risks in §9:

- land regions read from a sketch;
- copy indices of patterns;
- optional features driven by parameters;
- the authoring UI;
- rendering parity of a hand-authored template.

## 3. Solidcraft fork API

Work happens in an isolated fork worktree branched from `pcbcraft-via2`, following the fork's AGENTS.md:

- commands have input and output schemas;
- there is no unwrap or panic;
- input is hostile;
- `cargo xtask ci` runs before each commit;
- new UI strings go in `assets/locales/pending/`;
- any format change is one file in `crates/doc/src/format/pending/`.

Nothing is pushed to `origin`. Pushing to `fork` happens only after integration, as for earlier fork changes. Pcbcraft then pins the new revision.

### SC1 — Insert a design's history (required)

The Rust API, in a new file `crates/doc/src/insert_history.rs` plus a command in `crates/engine/src/cmd/insert_history.rs`:

```rust
pub struct InsertSpec<'a> {
    pub source: &'a Document,          // already read and evaluated by the caller
    pub source_bodies: &'a [String],   // the source's evaluated body names, in order
    pub component: Option<u64>,        // which source component to take; None = root
    pub name: &'a str,                 // the new component's name
    pub param_prefix: &'a str,         // "<prefix>_<name>" for every parameter, input and dimension
    pub body_prefix: &'a str,          // "<prefix> <name>" for bodies and features
    pub values: &'a [(String, String)],// overrides by source parameter name: literal quantities only
    pub parent: Option<u64>,
}
pub struct InsertReport {
    pub component: u64, pub occurrence: u64,
    pub features: BTreeMap<u64, u64>,        // source feature id → host id
    pub bodies: BTreeMap<String, String>,    // source body name → host body name
    pub parameters: BTreeMap<String, String>,
    pub constructs: BTreeMap<String, String>,// construction features by name
}
impl Session {
    pub fn insert_history(&mut self, spec: &InsertSpec) -> Result<InsertReport>; // one undo step, one evaluation
}
```

- **Command.** `component.insert_history { design: base64 | path, component?, name, param_prefix, body_prefix, values?, parent? }`, with a schema. The reply is the report. The JSON command exists for MCP and for Solidcraft users; Pcbcraft calls the Rust API. It is deferrable within `defer_evaluation` (it joins the `DEFERRABLE` handling), so a board inserts every definition before one evaluation.
- **Remap.** It uses `Feature::remap_clone` with complete maps, including the source's evaluated body names (patterns pin them) and `param_defs()`, which covers user parameters, feature inputs and sketch dimensions. Model parameters keep `model: true`. Source configuration rules (`ConfigRule`) are copied with their parameter names prefixed. Configuration rows are not copied: Pcbcraft turns them into presets.
- **Refusals**, leaving the host unchanged:
  - a source with subcomponents, occurrences, joints, body transfers, links or external face anchors (release 1 templates are single-component);
  - a name collision after prefixing;
  - an override naming an unknown or non-user parameter;
  - an override value with a parameter reference;
  - any source feature failing in the host. The insert is atomic, so a failed evaluation rolls back.
- **Plain Solidcraft.** Inserted features are ordinary features, so a board reopens in plain Solidcraft (shown by the spike's reopen).
- **Replace.** `component.replace_history { component, … }` replaces a definition's features from a new template revision under the same prefixes. It is used by library updates. Joints to the component keep their persistent face names where the faces survive and are reported where they do not.

### SC2 — References inside namespaced metadata (required)

- **Reference objects.** In a namespaced metadata value, an object with exactly one of these keys is a reference:
  - `{"$param": name}`
  - `{"$body": name}`
  - `{"$feature": name}`
  - `{"$sketch": feature, "$curves": [ids]}`
  - `{"$pattern": feature, "$id": sketch pattern id}`
- **Following renames.** `Document::rename_param`, `body.rename`, feature rename and `rename_body_keys` rewrite these references. This is generic and has no Pcbcraft semantics.
- **Validation.** `document.references` lists dangling references, which the Pcbcraft check reports.
- **Size.** Metadata keeps its limits (64 KiB per value, 256 keys, 256 KiB in all). The interface is chunked as `org.pcbcraft.part_template.0`, `.1`, …, each at most 48 KiB, like the model chunks today.

### SC3 — Read the evaluated interface (required)

These are pure queries on an evaluated `Session`, with Rust functions and commands for MCP:

- **Construction geometry.** `inspect.construction { names? }` returns construction points, axes and planes by name, from `ModelState.construct`.
- **Pattern copies.** `inspect.pattern_copies { feature }` returns `(copy index [i, j], body name)` for a body pattern, and `(index, curve ids)` for a sketch pattern (`SketchPattern.instances`).
- **Sketch regions.** `inspect.sketch_regions { feature }` returns each closed region of a solved sketch as loops of curve ids with exact lines and arcs, in the template frame.

### SC4 — The Pcbcraft material library (required, small)

Pcbcraft exports its material table as a Solidcraft local material library (`crates/engine/src/cmd/libraries.rs` format). Each entry's name is `pcbcraft:<material_id>` and carries the opaque PBR projection (`models::look`). Authors assign looks from it. Material slots are named appearances (`pcbcraft-slot:<slot>`).

### SC5 — Authoring UI (minimum viable version)

This is generic and in the fork:

- **Parameters panel.** An "Interface" column group: exposed, label, group, minimum, maximum, step or choices, and binding. It writes to the metadata namespace that the open document declares as its interface schema.
- **Attributes panel.** Namespaced attributes on the selected body, feature, sketch region, sketch pattern or construction feature, edited through a form generated from a JSON schema the namespace owner ships. Pcbcraft ships `org.pcbcraft.part_template.schema.json`. The panel shows dangling references from SC2.
- **No Pcbcraft-specific panel.** Pad-number previews, numbering checks and land rules come from `pcbcraft-cli part-template check FILE [--watch]`, which re-checks the file on every save (§6).

### SC6 — Persisting the full material look (later, upstream-bound)

`Look` gains an optional clear coat (`{factor, roughness, ior}`) and a surface pattern. This is one pending format file. Solidcraft's product look then renders the authored file as Pcbcraft does. Until then Pcbcraft draws the full look from the `MaterialId` (D8).

### What Solidcraft already offers, and the gaps

- **Topological naming.** History-based persistent face and edge names exist: `F2:top`, `@F7.2` for pattern copies, split pieces told apart by their neighbouring faces. Body names are stable and user-settable (`body.rename`). Construction features and joint origins are named.
- **No region names.** Sketch regions have no names. `ProfileSel::Curves` (loops of curve ids) is stable across parameter changes, and SC3 exposes it.
- **No general attribute store.** SC2 fills that gap without a format change.
- **No parameter interface.** `Param` holds name, expression, unit, comment and the `model` flag, with no range or exposure; SC5 stores the interface in metadata.
- **Design-wide parameters.** Parameters are global per design, so SC1 prefixes them.
- **What Pcbcraft avoids.** It never stores face names in the interface in release 1. Anchors and marking surfaces are construction features with extents given as expressions, which do not depend on topological naming. A face reference (`$face`) is a later addition, once a template needs a surface no construction feature can give.

## 4. Template format (`pcbcraft-part-template/1`)

The interface is one JSON object, chunked across `org.pcbcraft.part_template.N`. Reference objects are those of SC2. Expressions use Solidcraft's expression language over the template's parameter names, evaluated by Solidcraft at the requested values. Pcbcraft never evaluates an expression with its own evaluator.

Example, abridged, for a four-row shielded module:

```json
{
  "schema": "pcbcraft-part-template/1",
  "name": "Shielded module, four pad rows",
  "family_hint": "module",
  "frame": {
    "origin":  {"$feature": "Origin"},
    "seating": {"$feature": "Seating plane"},
    "pin1":    {"$feature": "Pin 1"}
  },
  "parameters": [
    {"param": {"$param": "pitch"}, "kind": "length", "label": "Pad pitch", "group": "Pads",
     "min": "0.3 mm", "max": "2.54 mm", "binding": "package", "describe": "e in the outline drawing",
     "form_key": "pitch"},
    {"param": {"$param": "rows_left"}, "kind": "count", "min": 0, "max": 64, "binding": "package"},
    {"param": {"$param": "corner_pads"}, "kind": "bool", "binding": "package"},
    {"param": {"$param": "can_finish"}, "kind": "choice", "choices": ["nickel_silver", "tin"],
     "binding": "render"}
  ],
  "optional": [{"feature": {"$feature": "Corner pads"}, "present": "corner_pads"}],
  "bodies": [
    {"body": {"$body": "Board"}, "role": "body", "material": "solder_mask_green"},
    {"body": {"$body": "Can"},   "role": "other", "material": {"slot": "can"}}
  ],
  "material_slots": [{"name": "can", "param": "can_finish", "default": "nickel_silver"}],
  "leads": [
    {"pattern": {"$feature": "Left pads"}, "key": "1 + i", "role": "termination"},
    {"pattern": {"$feature": "Ground pads"}, "key": "95", "role": "exposed_pad"}
  ],
  "land": {
    "sketch": {"$feature": "Land pattern"},
    "regions": [
      {"pattern": {"$pattern": "Land pattern", "$id": "p1"}, "number": "1 + i", "lead": "1 + i",
       "kind": "smd", "mask": "inherit", "paste": "inherit"},
      {"loops": {"$sketch": "Land pattern", "$curves": ["l40", "l41", "l42", "l43"]},
       "number": "95", "lead": "95", "kind": "smd", "mask": {"expansion": "0.05 mm"},
       "paste": {"windows": [{"$sketch": "Land pattern", "$curves": ["l50", "l51", "l52", "l53"]}]}}
    ]
  },
  "courtyard": {"derived": {"excess": "courtyard_excess"}},
  "keepouts": [{"region": {"$sketch": "Antenna keep-out", "$curves": ["l1", "l2", "l3", "l4"]},
                "layers": "all", "pads": "forbid"}],
  "marking": {
    "surfaces": [
      {"name": "can_top", "plane": {"$feature": "Can top"},
       "size": ["can_width - 2 * can_round", "can_length - 2 * can_round"],
       "describe": "the shield can's flat top"}
    ],
    "default": {"items": [{"item": "text", "template": "{mpn}", "height_mm": 1.25},
                          {"item": "artwork", "svg_slot": "logo", "anchor": "pin1_corner", "size_mm": [6, 4]}],
                "finish": "laser_etch_metal",
                "placement": {"on": {"surface": "anchor", "name": "can_top"}}},
    "slots": [{"name": "logo", "kind": "svg", "describe": "manufacturer logo"}]
  },
  "rules": [{"check": "pitch * (rows_left - 1) + pad_length <= length", "severity": "error",
             "message": "the left row is longer than the module"}],
  "presets": [{"name": "ESP32-S31-WROOM-3", "values": {"pitch": "0.85 mm", "rows_left": 25},
               "citation": {"document": "ESP32-S31-WROOM-3 datasheet v0.7", "page": "Figure 11-1"}}],
  "provenance": {"author": "…", "licence": "MIT OR Apache-2.0", "original": true, "sources": []}
}
```

### Parameters

- **Fields.** `kind` is `length | angle | count | bool | choice | text`. `min`, `max` and `step` (or `choices`) are required on every exposed parameter. `binding` is `package | instance | render`. Each parameter also has a label, a group, a description of where the value comes from in a drawing, and an optional `form_key` naming the matching typed-form field (D10).
- **Internal parameters.** A parameter not listed is internal. It can be read in expressions but never overridden.
- **Booleans and choices.** A `bool` or `choice` drives `optional` features by suppression (the insert copies them as host configuration rules, SC1) or a material slot (render).
- **Text parameters.** `text` parameters are render-only and feed marking slots.
- **Presets.** A preset is a named set of values with a citation, carried over from Solidcraft configuration rows or authored in the interface. The package-row catalogue can cite presets.

### Anchors

These are construction features:

- `origin`: the package datum, with x along the package length.
- `seating`: a plane, z = 0 of the model frame.
- `pin1`: a point, needed when the part is polarised.

Further named anchors become marking or quick-action targets. The model frame is the one generated models use today, with z = 0 on the seating plane ([M1–M3](component-model.md#generated-3d-models-and-placement-m1m3-as-built)).

### Lead contacts

- **Tagging.** A lead entry tags one body or a body pattern (the copy index `i`, `j` comes from SC3) with a terminal **key** expression and a role (`lead | termination | exposed_pad`).
- **Contact point.** The contact is the lowest geometry of the tagged body within `CONTACT_BAND_MM`. This is the rule imported models use, so `fit_leads`, `lead_off_land` and the 3D checks work unchanged.
- **Mapping to terminals.** Keys map to package terminals through an explicit `lead_map`, as for library models. The UI only proposes exact designation matches; nothing is inferred. Several bodies may share a key (a ground array lands one terminal).

### Land pattern

The land pattern is a sketch in the seating plane whose regions are tagged:

- **Region.** Each region is a loop of curve ids, or a sketch pattern with the instance index `i`, `j`. Its tags are:
  - `number`, a text expression;
  - `lead`, the key of the lead it lands, so the pad's terminal follows the lead map;
  - `kind` (`smd | plated_through | non_plated_hole | mechanical | fiducial`);
  - `mask` (`inherit | tented | {expansion}`);
  - `paste` (`inherit | none | {reduction} | {windows: [loops]}`);
  - `hole`, an inner loop or a drill expression.
- **Conversion.** Pcbcraft classifies each region with the footprint evaluator's existing profile pass (`crates/solidcraft-integration/src/footprint/`), which gives typed shapes or exact custom outlines. The pads are written into the footprint draft with the footprint's defaults and the board's rules, as `library.package_from_template` does today. Mask and paste are *intent*; the footprint lint (`footprint.mask_web`, paste windows inside pads) and the board checks apply to the result.
- **Unsupported curves.** A region that is not closed, or has a curve the profile pass cannot classify, is the refusal `part_template.land_region`, naming the curves.

### Courtyard and keep-outs

- **Courtyard.** It is `derived` (extents grown by an expression, the existing `CourtyardFromExtents`) or a tagged closed region.
- **Keep-outs.** Tagged regions with a layer scope (`all | component_side`) and pad rule (`forbid | allow`), mapped to `FootprintKeepout`. A module's antenna keep-out becomes template data instead of form code.

### Marking surfaces and slots

- **Surfaces.** A surface is `plane` (a construction plane, its size as expressions, the u axis along the plane's x) or `cylinder` (a construction axis, a radius expression, an `along` range and angle 0 as a construction point). They become `AnchorInfo { name, description, geometry: SurfaceGeometry::Plane | Cylinder }`, so layout, wraps, placement, clipping and decals work unchanged.
- **Default.** `default` is a `MarkingSpec` in the existing JSON (`library.set_marking`).
- **Slots and bindings.** Slots name the values a marking binds:
  - `{mpn}`, `{manufacturer}`, `{designator}` and `{var:…}` as today;
  - `{value_code}`, short for the component's first value field with `|code`, which gives a resistor's "101";
  - `{param:NAME}`, a render or text parameter's value as text;
  - `{svg:NAME}`, an artwork slot whose packed SVG comes from the Part, component or instance;
  - `{qr:…}`, the existing `code2d` item;
  - the polarity stripe and the electrolytic item, as today.
- **Precedence.** instance, then Part, then component, then the template default; a level that hides hides.
- **Variants.** One ESP32 module template with different Parts differs only in `{mpn}` and `{svg:logo}`.

### Validation rules

**Structural**, checked by Pcbcraft on import and publish, and by `pcbcraft-cli part-template check`. All are blocking unless marked:

- The schema version is known; every reference resolves (SC2).
- Every exposed parameter has a range, and its default lies in it.
- Every body has a role and a material (`part_template.body_unmapped`).
- Lead keys are present and every land `lead` names one.
- Pad numbers are unique per key rule.
- `origin` and `seating` exist, and `pin1` exists when a polarity mark or polarised marking is declared.
- Release 1 templates are single-component (SC1).
- A built-in template contains no import or mesh-import feature; a user template may, only with an import licence recorded (`part_template.import_licence`).
- The template evaluates without a failing feature at:
  - its defaults;
  - every preset;
  - each exposed parameter's minimum and maximum, one at a time.

  This is at most 64 evaluations (a target), or `part_template.range_fails`, naming the values.
- Advisory: `part_template.slow`, cold evaluation above 100 ms (a target).

**Authored rules** (`rules`) are Solidcraft expressions over the parameters, each with a severity and message. They are checked by Solidcraft's evaluator at the requested values and reported as `part_template.rule`, blocking at `error`.

**Bounds.** At most 512 KiB of interface, 256 parameters, 64 lead entries, 1024 land regions, 16 keep-outs, 16 surfaces, 32 slots and 64 presets. The asset itself is at most 64 MiB, the container limit. All are targets.

## 5. Pcbcraft integration

### Library kind (core, `library/part_template.rs`)

```rust
pub struct PartTemplateDefinition {
    pub asset: AssetRef,                    // the .solidcraft/.sc container, packed by hash
    pub interface: PartTemplateInterface,   // parsed metadata, re-derived and compared at publish/build
    pub engine: EngineRef,                  // Solidcraft format + fork revision it was checked with
    pub saved_model: bool,                  // the container carries its default-set model (§2)
    pub thumbnail: Option<AssetRef>,
    pub provenance: Provenance,             // author, licence, redistribution, sources
}
pub enum ParamValue { Length(f64), Angle(f64), Count(u32), Bool(bool), Choice(String), Text(String) } // mm, degrees
pub type TemplateValues = BTreeMap<String, ParamValue>;
pub struct ParamSetKey(pub ContentHash);
```

- **Rank and snapshot.** The kind is published at rank 2, like Model, and becomes `Definition::PartTemplate(Box<…>)`. The project snapshot carries the bytes; bundles, closures and promotion carry them too, subject to redistribution. Update plans list template changes (§5, updates).
- **Blocking findings.** The structural codes of §4. Also:
  - `part_template.asset`: the asset is missing or its hash does not match;
  - `part_template.stale`: the stored interface differs from the asset's;
  - `part_template.licence`;
  - `part_template.engine`: an unsupported Solidcraft format.
- **Evaluator seam.** The core does not depend on Solidcraft. It defines

  ```rust
  pub trait PartTemplateEvaluator {
      fn inspect(&self, bytes: &AssetBytes) -> Result<PartTemplateInspection>;     // interface, defaults, findings
      fn evaluate(&self, bytes: &AssetBytes, values: &TemplateValues, budget: Budget)
          -> Result<TemplateEvaluation>;
  }
  pub struct TemplateEvaluation {
      pub key: ParamSetKey, pub envelope: [f64; 3], pub bounds: [[f64; 3]; 2],
      pub anchors: Vec<(String, [[f64; 3]; 2])>,       // point and x/normal direction
      pub leads: Vec<LeadContact>,                      // key, contact point, body name, role
      pub lands: Vec<LandPad>,                          // number, lead key, kind, shape, position, rotation, mask, paste, hole
      pub courtyard: Vec<Shape>, pub keepouts: Vec<FootprintKeepout>,
      pub surfaces: Vec<AnchorInfo>, pub bodies: Vec<TemplateBody>, pub findings: Vec<Finding>,
  }
  ```

  The evaluator is passed through `Engine::apply_with` and `query_using` beside the `SketchEvaluator` (one evaluator bundle). `Engine::apply` without it refuses template edits with `part_template.evaluator_required`, as footprint sources do today.

### Packages and instances

- **Package.**
  - Form: `PackageForm::PartTemplate(Box<TemplateForm { template: DefinitionRef /* exact revision */, values: TemplateValues }>)`.
  - Model: `PackageModel::PartTemplate { materials: BTreeMap<String /* slot */, MaterialId>, lead_map: Vec<(String /* key */, PackageTerminalId)> }`.
  - Envelope: derived from the evaluation and stored, then rechecked at build (`RECHECK_TOLERANCE_MM`).
  - `pkg_*` values: `pkg_<name>` for every exposed numeric package parameter, so footprint expressions keep working.
  - Lead points: from the evaluation's leads, so `fit_leads` and placement quick actions work unchanged.
- **Instance.** `InstanceTemplate { values: TemplateValues }` holds overrides of `instance`-binding parameters only (D5). An instance's definition is `pkg<id>r<rev>` for the package values and `pkg<id>r<rev>v<8 hex of ParamSetKey>` for each distinct override set. At most 8 distinct sets per package per board is a target and a finding beyond it (`part_template.instance_sets`).
- **Render-binding parameters** live in the marking (`InstanceMarking.vars`) and material slots. They never change a `ParamSetKey`.

### Commands

These are typed, one undo step each, and failures leave the document unchanged. CLI and MCP schemas are generated from them.

| Kind | ID | Payload → result |
|---|---|---|
| Library command | `library.import_part_template` | `{ bytes \| asset, name, provenance }` → a PartTemplate draft (inspected, interface stored). With `assign: { package, values, lead_map }` it also sets the package draft, in the same step. |
| Library command | `library.package_from_template` (extended) | Row source `{source: part_template, template, values, preset?}`: package form, model, terminals by key, and derived lands in a new or the landing footprint draft, in one step. |
| Library command | `library.edit_package` (extended) | `PackageEdit::SetTemplateValues { values }`, `SetTemplateMaterials`, `SetLeadMap`. Lands are re-derived only while the footprint is still derived from this template (D6). |
| Library command | `library.edit_part_template` | Metadata-only edits Pcbcraft may make: presets, description, provenance. Geometry and interface are edited in Solidcraft, then re-imported as a new revision. |
| Instance command | `instance.set_template_values` | Instance overrides; refuses non-`instance` parameters. |
| Query | `library.part_template_interface { template }` | The parameter schema with ranges, groups and descriptions, plus anchors, slots and presets: what an agent reads. |
| Query | `library.part_template_plan { package, template, values }` | Evaluated summary and changes (envelope, leads, pads, keep-outs, findings) without geometry. |
| Query | `library.templates` (extended) | Hits of kind `part_template`, facets `kind:`, `family:` and `licence:`. |
| Automation | `library.inspect_part_template_file { path }`, `part-template check` (CLI) | A file's interface and findings. The CLI's `--watch` re-checks on save. |

### Build and cache (integration, `models/template.rs`, `models/template/{inspect,land,surfaces}.rs`)

- **Board.** `project::build` inserts each distinct `ParamSetKey` once with SC1 (`param_prefix = pkg<id>r<rev>[v…]`). It does so inside the existing `defer_evaluation` batch and places occurrences as today. Bodies get role, material and terminal from the interface and lead map, stored in the existing `org.pcbcraft.electronics.models.N` chunks. Looks are assigned through `appearance.assign`, so the saved model is parametric and plain Solidcraft opens it.
- **Replay.** The existing replay log (`native/replay.rs`) passes a definition whose `ParamSetKey` and template hash are unchanged (`Native::pass`), so an unrelated board edit reruns nothing.
- **Preview cache.** `TemplateCache`, an LRU of at most 32 parameter sets and 64 MiB (targets), keyed by `ParamSetKey`. It holds the evaluated `TemplateEvaluation` and the tessellated bodies for the Component 3D preview and the parameter form. The default set loads through `Model::adopt` from the template's saved model (under 10 ms measured).
- **Threading.** All evaluation runs on the model worker, cancellable and revision-tagged. A stale result never overwrites a newer request. The time budget per evaluation is 10 s, a target; beyond it the result is `part_template.eval_budget`.
- **Markings.** `marking/board.rs` takes surfaces from the evaluation when the package form is a template; otherwise from `surface::anchors(form)`. The decal cache is unchanged.

### Parameter UI (desktop)

- **Component 3D tab, Body section.** The family list gains **Part template**, with a chooser: the gallery sheet's thumbnails, filtered to part templates.
  - Parameters appear in their groups as kit fields with units, ranges and the invalid look, plus a Preset dropdown. Leaving a field sends one `SetTemplateValues`.
  - The preview re-evaluates off the UI thread, latest request wins, with the previous model shown until the new one arrives.
  - Fields of `render` parameters update the marking preview only.
- **Import wizard** (`library.import_part_template`): file, interface report with findings, licence, review. It follows the model import wizard's layout.
- **Instance card.** Overridable parameters and marking slots, one command each.

### MCP and the datasheet assistant

Every command and query above is an MCP tool through `project_apply` and `project_query`. The assistant's tool set (`crates/assist/src/tools.rs`) adds:

- `library_templates` with `kind:part_template`;
- `library_part_template_interface`;
- `library_part_template_plan`;
- `library_package_from_template` with the `part_template` source.

The server instructions describe the flow: choose a template by `family_hint` and drawing, fill each parameter from a cited dimension, preview the plan, apply, set the marking slots. Every value goes into `submit_review` with its page, as today. A replay test (`wroom3_template`) runs the WROOM-3 through the module template.

### Package forms and rows

- **Forms.** A typed form maps to its built-in template by a pure core function (`templates::builtin::values_of(&PackageForm) -> (TemplateRef, TemplateValues)`), using `form_key`. Package rows keep storing typed forms. A row may instead store `PackageForm::PartTemplate` with a cited preset.
- **Footprint templates.** `FootprintTemplate` (F5) stays as it is. A template-derived footprint is an alternative source for the same draft.

### Updates

- **Impact preview.** A new template revision is a library update with an impact preview: envelope change, lead keys added or removed, pads moved, reshaped, added or retired (by derived pad identity: key and number), and marking surfaces renamed.
- **Applying.** It applies atomically (SC1 replace) or not at all. A derived pad keeps its `PadId` while its key and number survive.

### Clean room and provenance

- **Built-in templates** are original work authored in Solidcraft from public drawings. Each value is cited in its preset; every built-in is `"original": true` with no import features. They live in `library/part-templates/*.sc` with a provenance row each.
- **User templates** may contain imported manufacturer STEP (published CAD data used like a datasheet, [libraries](../skills/pcbcraft-libraries/SKILL.md#clean-room-sources)), with its licence recorded. Their redistribution follows it.
- **CI.** `cargo xtask clean-room` gains a check that every tracked `.sc` or `.solidcraft` under `library/` carries `org.pcbcraft.part_template` provenance and contains no import feature. Templates must never be converted from EDA tool libraries.

## 6. Authoring in Solidcraft

1. **Model the part** in Solidcraft with named user parameters for every dimension a package varies. Build with patterns and disjoint bodies, not many-tool cuts (§2), and keep it single-component.
2. **Declare the interface** in the Parameters panel's Interface columns (SC5): exposed, range, group, label, binding.
3. **Add anchors**: construction point `Origin`, plane `Seating plane` and point `Pin 1`, using existing commands. Tag them in the Attributes panel.
4. **Tag the leads**: select a body or body pattern, then in Attributes set role and key expression (`1 + i`).
5. **Draw the land pattern** as a sketch on the seating plane. Patterns of a pad region become pad rows. Tag each region or pattern with number, lead, kind, mask and paste. Add courtyard and keep-out regions in their own sketches.
6. **Add marking surfaces**: construction planes or axes with size expressions. Edit the default marking and slots in the Attributes form.
7. **Assign looks** from the Pcbcraft material library (SC4), or a material slot.
8. **Check and save**: run `pcbcraft-cli part-template check part.sc --watch` beside Solidcraft. It reports pad numbering, lead and pad pairing, range corners and timing on every save. Save as a container with the model cache, then import in Pcbcraft.

**Minimum viable versions:**

- **v0**: no fork UI. Steps 2–6 are done over Solidcraft MCP (`document.metadata.set`) by an agent or script, checked by the CLI. This is enough for the module proof (F1 in §9).
- **v1**: the SC5 panels. A person can author without JSON.
- **v2** (later): `$face` references; a land preview drawn in Solidcraft from the check's output; SC6 looks.

## 7. Migration and parity

### How parity is measured (`models/template/parity.rs`, test-only)

The template evaluated at a form's values is compared with the generated build of the same form:

1. **Bodies.** Paired by role and terminal, else by nearest centroid within the role. Counts per role are equal, every lead terminal is present, and materials are equal.
2. **Geometry.**
   - Per pair: bounding box within 0.005 mm and volume within 0.5 %.
   - Per pair: symmetric Hausdorff distance between tessellations within 0.02 mm, sampled at the vertices of both meshes.
   - Union bounds within 0.005 mm.

   Hand-authored templates may add detail. A body the generator lacks, or a deviation above tolerance, is listed and must be accepted explicitly by name in the parity fixture (`accepted_differences`), never by widening a tolerance.
3. **Electrical geometry.**
   - Lead contacts (`lead_points`) within 0.001 mm.
   - Derived pads equal to the form's lands (`samples::form_lands` or `ModuleForm::lands`): position and size within 0.001 mm, same kind, mask and paste.
   - Keep-outs equal.
   - Marking surfaces equal to `surface::anchors(form)` within 0.001 mm.
4. **Pictures.** Rendered headless, side by side, at 1024 × 768 and 2× supersampling from the existing two camera poses (35°/32° and 215°/−30°) through the product look.
   - Pass: mean absolute luminance difference at most 1/255, and the 99th percentile at most 8/255.
   - Both PNGs and a difference image are written under `PCBCRAFT_PICTURES` for review.

   The harness reuses only public viewport items (`Viewport::detached`, `model_scene::build`). It lives in a new file outside `apps/pcbcraft/src/viewport/` while the 3D view fixes are in flight. Sharing `pictures.rs`'s GPU helper waits until those fixes merge.
5. **Cost.** Cold and cached evaluation are recorded against the generated build (§2 numbers as the baseline). The gate is only that a template is not more than 2× slower cold, a target.

### First proof: the module family and the WROOM-3

- **The template.** "Shielded module, four pad rows" is authored in Solidcraft. Its parameters cover the `ModuleForm` fields (`form_key`):
  - pad rows per side with first position;
  - a ground array (rows, columns, pitch, turn);
  - four optional corner pads;
  - the can, the antenna region and castellation diameter;
  - land width and extension.

  Its land pattern, keep-out and the `can_top`, `can_*`, `pcb_top` and `antenna_area` surfaces come from the template rather than from Rust.
- **Criterion.** The ESP32-S31-WROOM-3 built from the template at the example project's form values (`examples/esp32-s31-wroom-3/`, also `tests/template_spike_wroom3.json`) passes every check above against the generated module. Specifically:
  - 110 bodies paired;
  - 99 terminals;
  - 107 pads equal to the form's lands;
  - the antenna keep-out equal;
  - the marking of `examples/esp32-s31-wroom-3/marking.json` laid out identically;
  - both pictures within tolerance.

  The founder reviews the pictures before the family switches.
- **Range.** The same suite passes for the starter module rows and for a synthetic castellated module (18 × 25.5 mm, castellation 1.0 mm). This shows the template is parametric, not tuned to one part.
- **Improvement.** After parity, the WROOM-3's chamfered EPAD pad and the can's Ø0.5 corner mark (both listed as limits in [datasheet assistant](datasheet-assistant.md#known-limits)) become template detail. They are accepted differences, recorded by name.

### The remaining families

Chip, gull-wing, no-lead, header, tabbed, molded, grid and THT follow, one built-in template each (or a few where the generator branches heavily: gull-wing two- and four-side, THT radial and axial). Each family:

1. Author the template.
2. Run the parity suite over **every starter row of the family** (196 rows in all today).
3. Have the founder review the pictures for one representative row.
4. Switch the family's `build_definition` to the template behind `values_of`. Pre-release, so with no migration: a project's generated model simply builds through the template.
5. Delete the family's Rust builder once its parity test has been green on main for one integration cycle.

`family_rev` is retired with the last builder. Rollback for any family is reverting its switch commit; rollback for all of this is the tags.

## 8. Upstream PR plan (to tangency-labs/solidcraft)

This is a plan only. Opening any PR needs the founder's explicit go.

**The fork today.** `pcbcraft-via2` is 9 non-merge commits ahead of the merge base 4f3ac07ae. That is 47 files changed, +5603 and −166, with no format change; upstream `main` has moved since.

**Order.** One topic per PR, each rebased on the latest `origin/main`, smallest and least controversial first:

1. **Linear-time drilled boards.** `geom/box_index.rs`, indexed profile nesting in `sketch/profiles.rs`, `kernel/prism_mesh.rs`, children map in `placements()`, `set_param` without a document clone, and `expr::eval_params_for`. It has nothing visible, so it needs no screenshots; it carries the `eval_cost_tests` evidence and timings.
2. **Batched copies and joints.** `occurrence.copy { translations }` and `joint.create { joints }`. Each needs an input and output schema: both commands are in `schema/unconverted.txt`, which must only shrink.
3. **Put-off evaluation.** `Session::defer_evaluation` and `evaluate_deferred`, and the deferrable sketch and extrude forms. Upstream may want it as a command (`edit.batch`); offer the Rust API with tests and ask.
4. **Opt-in product look.** The commits 4477d4e78 "Add an opt-in product look", 03c13afc1, 06833dbb2, 87b1009b0 and 4176c0d84: `crates/gpu/src/view/{ibl,ao,msaa_check}.rs`, `pbr.rs`, `viewport.wgsl`, `ui-egui` toggles, `docs/render-opaque-pbr.md` and `docs/render-clear-coat.md`.
   - It is opt-in, so `Changes a default:` reads "none".
   - It needs real before and after screenshots, captured from a public-safe sample design.
   - New UI strings go in `assets/locales/pending/`.
   - The Khronos PBR Neutral tone mapper (Apache-2.0) keeps its `attribution/pending/pbr-product-look.md` row and NOTICE paragraph.
5. **Insert a design's history** (SC1) and **references in namespaced metadata** (SC2): two PRs with schemas and tests. These are generic and useful to Solidcraft's own library parts.
6. **Inspection queries** (SC3).
7. **Persisted clear coat and pattern** (SC6): one `format/pending/` file, plus screenshots.
8. **Parameter interface and attributes panels** (SC5), only if upstream wants an interface concept; otherwise they stay in the fork.

**Before each PR:**

- Read upstream AGENTS.md and the contributing skill again.
- List open PRs and issues (`gh pr list -R tangency-labs/solidcraft --state open --limit 500`, and the same for issues), checking for overlap.
- Claim with a draft PR (`claim: #N`, lowest number wins).
- Run `cargo xtask ci` without switching branches.
- Do not edit generated files (`docs/features.md`, `THIRD-PARTY-LICENSES.txt`). Put each regression test in the area's existing test file.
- Never force-push `main`, and do not push just to catch up.

**Licence.** All fork work is our original contribution under Solidcraft's MIT OR Apache-2.0. Nothing Pcbcraft-specific is proposed: no material table, no `org.pcbcraft` schema. The Pcbcraft material library file stays in Pcbcraft.

## 9. Work breakdown

### Contract commit (C0)

This note, plus stubs that compile with refusals:

- the core types of §5 (`PartTemplateDefinition`, `TemplateForm`, `ParamValue`, `TemplateValues`, `ParamSetKey`, `PartTemplateEvaluator`, `TemplateEvaluation`);
- the new `Definition`, `PackageForm` and `PackageModel` variants;
- command IDs registered;
- finding codes in `issues::KNOWN_CODES`;
- the core module map rows in [component model](component-model.md#d-module-map), which `cargo xtask core-modules` enforces;
- the JSON schema `library/part-templates/org.pcbcraft.part_template.schema.json`;
- the spike's `insert_history` as the interim SC1, behind a function with SC1's signature in the integration crate. Tracks C and F can then start before SC1 lands.

### Tracks

They are parallel after C0. Hours are agent-hours.

| Track | Owns | Contract it relies on | Est. |
|---|---|---|---|
| **A — fork** (isolated fork worktree) | SC1 `insert_history` + command + schemas (8), SC2 metadata references (5), SC3 queries (5), SC4 material library (4), SC5 panels v1 (14) | §3 signatures | 36 h |
| **B — core** | `library/part_template.rs`, `…/part_template/{interface,checks,values}.rs`; package and instance variants; commands and schemas (22); marking surfaces and slot bindings in `library/marking/` (6) | §4 JSON, §5 types | 28 h |
| **C — integration** | `models/template.rs`, `models/template/{inspect,land,surfaces,parity}.rs`: evaluator (10), land conversion (6), board build and cache and replay (10), parity harness (6) | SC1–SC3 signatures (interim spike code), §5 types | 32 h |
| **D — desktop** | `apps/pcbcraft/src/part_editor/template_*.rs`, gallery filter, import wizard (6), parameter form, presets and preview (8), slots and instance rows (5) | §5 commands; C's preview cache API | 19 h |
| **E — MCP and assistant** | `crates/assist/src/tools.rs` entries, server instructions, `wroom3_template` replay test | §5 commands and queries | 6 h |
| **F1 — module template** | `library/part-templates/module-four-rows.sc` authored in Solidcraft (v0 over MCP), parity fixture, pictures | §4 schema, C's check and parity | 10 h |
| **F2 — other families** | one template each (8 families, about 10 templates) and their parity fixtures | after F1 | 48 h |
| **G — upstream PRs** | the eight PRs of §8, after the founder's go | — | about 30 h |
| **Integration and review** | merge, CI, as-built folding into the component model doc | — | 8 h |

**Totals.**

| Scope | Total | Critical path |
|---|---|---|
| The module proof (A–E, F1, integration) | 139 agent-hours | C0 → C evaluator and build → F1 → parity → integration, about 45 agent-hours with stubs |
| All families (adding F2) | 187 agent-hours | — |
| With the upstream PRs (adding G) | about 217 agent-hours | — |

**No overlap with parallel work.** No track touches routing (`layout/route*`, push-and-shove), the 3D view fixes (`apps/pcbcraft/src/viewport/*`, `scene.rs` render paths) or the keyboard abstraction. Track C touches `project.rs` only at the build hook and `marking/board.rs` only at surface lookup.

### Risks

| # | Risk | Mitigation |
|---|---|---|
| R1 | The insert's remap misses a reference kind (a feature type whose references `remap_clone` does not walk). | SC1's tests insert every feature kind Solidcraft's own `component.paste_new` tests cover. The insert refuses unknown references instead of leaving them dangling. Release 1 templates are single-component. |
| R2 | Land regions from sketches: classification, the copy index of sketch patterns, holes and paste windows inside regions. | Reuse the footprint evaluator's profile pass. SC3 exposes pattern instances, which Solidcraft already stores (`SketchPattern.instances`). A refusal names the curves. Fallback: per-region tags without patterns. |
| R3 | Visual parity of hand-authored templates. A generator fillet picks edges by points, a template by persistent names, which may round differently at range corners. | The parity suite runs at every starter row. Accepted differences are recorded by name; tolerances are never widened. |
| R4 | Evaluation cost of Boolean-heavy templates (QFN-48: 1 s). | Deduplicate by `ParamSetKey`; ship the saved model; the `part_template.slow` lint; authoring guidance; the cost gate in parity. |
| R5 | Board file size: history per definition (WROOM-3 about 117 KiB of JSON each). | The container compresses. The number of distinct definitions is the number of distinct packages, not of instances. Measure on the large benchmark board before release. |
| R6 | Fork divergence: SC1 builds on internals (`remap_clone`, `param_defs`) upstream may change. | Upstream it early (§8 step 5). SC1's tests live in the fork and run in its CI. |
| R7 | Untrusted templates (hostile expressions, huge patterns). | Size bounds, evaluation budget and cancel. Literal-only overrides (D4). Solidcraft's own hostile-input checks. |
| R8 | Authors without the v1 panels cannot work alone. | v0 is enough for built-ins (agent-authored). v1 is planned in track A. |

### Open decisions for the founder

1. **O1: typed forms.** Keep them as typed views of built-in templates (recommended), or replace them with the generic template form everywhere.
2. **O2: where the interface lives.** Namespaced metadata with references (recommended: no format change, upstream-compatible), or a native Solidcraft "design interface" format change.
3. **O3: land pattern.** Derived pads (recommended for release 1), or a live parametric `FootprintSource` linked to the template.
4. **O4: per-instance geometric overrides.** Allow `instance` binding (each set its own definition, at most 8 per package), or keep geometry package-only in release 1 and let instances vary only render parameters.
5. **O5: fork authoring UI.** Generic panels v1 (recommended, upstreamable), or a Pcbcraft-specific template panel in the fork with live pad previews.
6. **O6: the upstream PRs.** Go or no go, and whether to start with the performance PR (recommended) or the product look.
7. **O7: material persistence.** Propose clear coat and pattern in Solidcraft's `Look` upstream (SC6), or keep the `pcbcraft:<material_id>` naming as the only binding.
8. **O8: imported geometry in user templates.** Allow a vendor STEP inside a user template when its licence is recorded (recommended), or forbid import features in all templates.
9. **O9: file extension.** Templates saved as `.sc` (upstream's current default) or `.solidcraft` (Pcbcraft's convention so far).

## 10. Out of scope

- Multi-component templates (subassemblies, moving parts), and joints inside templates.
- Freeform marking surfaces ([D7 next step](component-model.md#markings-on-any-surface-d7-as-built)).
- Data Matrix codes.
- Template editing inside Pcbcraft: geometry is always edited in Solidcraft.
- Live two-way sync between an open Solidcraft template and Pcbcraft. Re-import makes a new revision; `--watch` only checks.

## 11. Lean parts: contract

Contract, 3 November 2026, for the founder's revision above. For release 1 it replaces §3 SC2–SC5, §4 and the `PartTemplate` kind of §5. A **part file** is an ordinary parametric Solidcraft design (`.solidcraft`). A package uses one by naming it and giving values for some of its parameters. Pcbcraft builds the 3D body through Solidcraft's engine and reads the marking surfaces from named construction planes. The footprint, terminals and keep-outs stay the package's own (package form, datasheet assistant). Release 1 varies geometry per package only; instances vary by render-time markings.

### L1. The package's reference

- **Asset.** The part file is a Model library definition (`library/model3d.rs`) with `format: solidcraft`: an exact revision whose `asset` pins the file's SHA-256 and whose provenance records author and licence. Snapshots, bundles, promotion and update plans carry it as they carry any Model. A Solidcraft-format model maps no bodies (`bodies` may be empty): its looks come from appearance names (L2).
- **Package model.** `PackageModel::PartFile(PartFileModel)`:

  ```rust
  pub struct PartFileModel {
      pub model: ModelRef,                        // exact Model revision, format solidcraft
      pub values: BTreeMap<String, PartValue>,    // by the part's own parameter name
      pub derived: PartDerived,                   // derived at assignment, rechecked at build
  }
  pub enum PartValue { Mm(f64), Deg(f64), Count(u32), Number(f64) } // JSON {"mm": 3.1}
  pub struct PartDerived {
      pub bounds_mm: [[f64; 3]; 2],               // union of the bodies, model frame
      pub bodies: u32,
      pub surfaces: Vec<AnchorInfo>,              // marking surfaces (L2), model frame
  }
  ```

  Values are literals only: a value never names another parameter. A name must be an existing user parameter of the file (not a feature input or dimension), and the unit must match the parameter's (`mm` length, `deg` angle, none for counts and numbers). At most 256 values; names are at most 64 bytes of `[A-Za-z0-9_]`.
- **Derived data.** `derived` is what the part evaluated to when it was assigned, like an imported model's bounds and lead contacts. The board build evaluates the part again. It refuses with `part_file.stale`, naming the package, when bounds or surfaces differ by more than `RECHECK_TOLERANCE_MM` (0.01 mm) or the body count differs. The core uses `derived.surfaces` for marking layout and `library.marking_surfaces`, so it never evaluates Solidcraft.
- **Identity of a build.** `PartKey = sha256(asset hash ‖ canonical JSON of values)`. Packages with the same key share one native definition, named `pkgp<first 12 hex of the key>`, with parameters `pkgp…_<name>` and bodies `pkgp… <name>`.

### L2. Naming conventions and the check

A part file follows these conventions, all over generic Solidcraft features:

| Convention | Rule |
|---|---|
| Single component | The part's features live in the root component: no subcomponents, occurrences or joints. |
| Frame | The design origin is the footprint origin; z = 0 is the seating plane; x runs along the package length. The lowest geometry lies at z = 0 (±0.01 mm), and the origin lies inside the xy bounds. |
| Parameters | Plain user parameter names (`can_height`, `body_height`, `rows_left`). Any user parameter may be given a value; the rest are internal. |
| Materials | Each body's Solidcraft appearance is named after a Pcbcraft material: its `MaterialId` in kebab case (`nickel-silver`, `solder-mask-green`, `gold`, `epoxy-mould`, …; `pcbcraft-cli part check --materials` lists them). Case and `_`, `-` or space are not told apart. A body with any other appearance, or none, is a blocking finding. The Solidcraft look keeps Pcbcraft's opaque PBR projection, so plain Solidcraft shows it right; Pcbcraft draws its own full look for the material. When the fork's material graph lands, a graph material named the same way maps the same way (only `part_file::material_of` changes). |
| Marking surfaces | A construction plane named `marking-<name>` is a marking surface. Its origin is the area's centre, its x axis the area's u (reading) direction, and its normal points out of the part. Its size is the two length parameters `marking_<name>_u` and `marking_<name>_v` (`-` in `<name>` becomes `_`). It becomes the anchor `<name>` with `-` turned into `_` (`marking-can-top` → `can_top`). `marking-top` is the default marking area (`placement.on: model`); without it, the default is the first surface. |
| Anchors | A construction point named `pin1` marks pin 1. It is optional and reported by the check. Other named construction points are listed but not used in release 1. |
| Roles | A body's role follows its material: a lead finish (`gold`, `tin-matte`, `tin-bright`, `solder-sac`, `nickel`, `copper-alloy`, `phosphor-bronze`, `brass`) is a termination, drawn as a lead; anything else is the body. This is display only: a part file carries no lead tags, and terminals and pads stay the package's. |

**The check** evaluates the file at given values and reports findings with these codes. All block assignment and build unless marked advisory:

- `part_file.read`: unreadable, too large (the 64 MiB asset bound) or an unsupported Solidcraft format;
- `part_file.structure`: subcomponents, occurrences or joints;
- `part_file.value`: a value names an unknown or non-user parameter, or its unit does not match;
- `part_file.feature`: a feature fails at the values (the first 16 are named);
- `part_file.empty`, `part_file.bodies`: no bodies, or more than 1024;
- `part_file.seating`: the lowest geometry is not at z = 0;
- `part_file.origin` (advisory): the origin is outside the xy bounds;
- `part_file.material`: a body's appearance names no Pcbcraft material (the bodies are named);
- `part_file.marking`: a `marking-*` plane without both size parameters, a size that is not positive, or two surfaces with one anchor name;
- `part_file.no_marking` (advisory): no marking surface;
- `part_file.import` (advisory): the file contains an import feature; its source licence must be recorded on the Model;
- `part_file.slow` (advisory): the cold evaluation took more than 100 ms (a target).

The automation layer reports these codes for a package's part file. The core adds two library findings: `part_file.model`, for a model that is missing or not of format `solidcraft`, and `part_file.values`, for malformed values or derived data.

### L3. Build and cache API (`pcbcraft_solidcraft::models::part_file`)

```rust
pub type PartValues = BTreeMap<String, PartValue>;
pub struct EvaluatedPart {           // shared, immutable
    pub key: [u8; 32],               // PartKey (L1)
    pub parameters: Vec<PartParameter>,   // name, expression, unit, value as evaluated
    pub bodies: Vec<PartBody>,       // source name, MaterialId (None: unmapped), bounds
    pub surfaces: Vec<AnchorInfo>,
    pub pin1: Option<[f64; 3]>,
    pub bounds_mm: [[f64; 3]; 2],
    pub findings: Vec<PartFinding>,  // code, blocking, message (L2)
    pub evaluate_ms: f64,
}
pub fn evaluate(bytes: &[u8], values: &PartValues) -> Result<Arc<EvaluatedPart>>; // cached
pub fn derived(part: &EvaluatedPart) -> PartDerived;
pub fn insert(native: &mut Native, stem: &str, part: &EvaluatedPart) -> Result<ModelDefinition>;
pub fn cache_counts() -> (u64 /* hits */, u64 /* evaluations */);
```

- **Evaluate.** It reads the bytes with Solidcraft's reader, applies the values as literal parameter expressions and evaluates once. It then reads the bodies, appearances, construction geometry and parameters. It refuses `part_file.read` and `part_file.value` problems; every other finding is reported in the result.
- **Cache.** A process-wide LRU of at most 32 evaluated parts, keyed by `PartKey` (a target bound). Equal packages, the Component 3D preview, the check and every board build share it. A hit costs a SHA-256 of the bytes and the values (about 0.1 ms for the 152 KiB WROOM-3 file) and a lookup.
- **Insert.** It adds the evaluated part's history to the board design as component `stem`; this is the spike's typed insert. Every root feature is cloned with `Feature::remap_clone` over complete maps: feature ids, feature names → `<stem> <name>`, body names → `<stem> <name>`, and every `param_defs` name → `<stem>_<name>`. User parameters are copied with `clone_expression`, then the design is evaluated once.
  - It refuses a name collision or any inserted feature failing. It runs inside the board build, which is discarded whole on any failure, so a refusal leaves no partial board.
  - Looks are assigned through `appearance.assign` from the material table. Bodies are recorded in the `org.pcbcraft.electronics.models.N` chunks, so the saved `.solidcraft` board stays parametric and opens in plain Solidcraft.
- **Fork.** The insert uses only the pinned fork's public API (`Session::doc_mut`, `Feature::remap_clone`, `Document::param_defs`, `clone_expression`). The fork's generic `component.insert_history` (SC1) is prepared on the fork branch `insert-history`: any CAD user inserts a design's history with deterministic name maps, prefixed parameters and literal overrides, as one undo step, atomically. Once it is pushed and pinned, `insert` calls it instead, with no change to this contract.
- **Board.** `project::build` defines each distinct `PartKey` once, before the parts, inside the existing put-off evaluation, and places occurrences as for generated models. A replayed build passes a definition whose key is unchanged.
- **Threads.** Evaluation runs where board builds and previews run (the model worker); the UI thread only reads results. The automation and CLI calls are synchronous by nature.

### L4. Commands (one engine for UI, CLI and MCP)

| Kind | ID | Payload → result |
|---|---|---|
| Core library command | `library.edit_package` `PackageEdit::SetModel(PackageModel::PartFile(…))` | A package draft uses a part file: shape-checked (`part_file.values`), one undo step. |
| Core library command | `library.import_model` with `assign.part` | `{ name, assets, definition (format solidcraft), assign: { package, lead_map: [], part: { values, derived } } }`: the Model draft and the package's part-file model in one step. |
| Automation command | `library.use_part_file` | `{ package, path \| builtin, values?, name?, expected_revision? }` → the check report and the new revision. It reads the file (bounded) and checks it, refusing blocking findings. It then applies `library.import_model` with `assign.part`, or `SetModel` when the same asset is already a Model in the project. |
| Automation command | `library.set_part_values` | `{ package, values, expected_revision? }` → report and revision. It evaluates at the new values and applies `SetModel`. |
| Automation query | `library.check_part_file` | `{ path \| builtin, values? }` → the report: parameters, bodies, materials, surfaces, pin 1, bounds, findings, timing. |
| CLI | `pcbcraft-cli part check FILE [--set name=value]… [--json] [--materials]` | The report; exit status 1 on a blocking finding. |
| MCP and assistant | `library_check_part_file`, `library_use_part_file`, `library_set_part_values` | The automation requests above, as tools. |

Built-in part files ship under `library/parts/` and are named by `builtin` (`esp32-s31-wroom-3`). They are original work (MIT OR Apache-2.0), authored through Solidcraft's commands by `cargo test -p pcbcraft-solidcraft --test author_parts -- --ignored`.

### L5. As built (3 November 2026)

- **Code.** Core: `PackageModel::PartFile(Box<PartFileModel>)`, `PartValue`, `PartDerived` and the findings in `library/package_model.rs` and `library/model3d.rs` (`part_findings`); `surface::package_anchors` gives a part file's surfaces to marking layout, `library.marking_surfaces` and the Component 3D marking tab. Integration: `models/part_file.rs` (evaluate, cache, check, insert, `role_of`, `material_of`, built-ins) and `project::{define_part, part_bytes, preview_part}`. Automation: `session/part_files.rs`. CLI: `pcbcraft-cli part check`. Assistant: `library_check_part_file`, `library_use_part_file`, `library_set_part_values`, and one sentence in the prompt.
- **Roles.** A body's role follows its material (L2); the WROOM-3 part draws its 107 pads and castellations as leads and its board, can and antenna as the body, as the module builder did.
- **First part.** `library/parts/esp32-s31-wroom-3.solidcraft` (152 KiB JSON, 24 user parameters, 110 bodies, 8 marking planes and `pin1`). It is authored by `tests/author_parts.rs` through Solidcraft's commands (`parameters.import`, `appearance.assign`, `construct.plane.offset`, `construct.point.vertex`) and the public remap API, starting from the module builder's construction. Only what the builder drives by parameters is parametric (board size and thickness, pad thickness, can size, height and round); the pad rows, notches and antenna stay as drawn for the WROOM-3.
- **Example.** `examples/esp32-s31-wroom-3` uses it (replay step 06b, `write_the_example_part_file`).
- **Parity** (`part_file_tests.rs`): 110/110 bodies; worst bounding-box difference 0 mm; worst relative volume difference 0; total volume 1852.0734 mm³ both; union bounds equal; materials and drawn kinds equal; the eight marking surfaces equal `surface::anchors(form)` within 1e-9 mm, and the default area is the can (21 × 23 mm), as before. Pictures: the `write_wroom_board_pictures` set renders the board from the part file.
- **Cost** (release, Apple M5 Max, `tests/part_timings.rs`, three runs): module builder 103–122 ms; part file cold (read, apply values, evaluate, inspect) 45–47 ms, of which Solidcraft's evaluation is 43–45 ms; cached 0.27 ms; insert into a board 47–49 ms. Equal keys share one definition in a board.
- **Fork.** `component.insert_history` is on the fork branch `insert-history` (commit 7442748af, on `pcbcraft-via2` d53b131), not pushed and not pinned: Pcbcraft's insert uses the public API at the current pin, so merging this work needs no fork push. Switching `part_file::insert` to the command is a later, contract-neutral change once the fork is pushed and pinned.
- **Open.** No desktop chooser for part files yet (UI, CLI and MCP share the automation commands; the Body card shows a part file's name, licence and size). Instances vary by markings only (release 1).
