# Physical looks of the PCB 3D model

The PCB 3D view and the saved `.solidcraft` model use the same physical looks. The board's [fabrication attributes](#fabrication-presets) (mask colour and finish, pad finish, silkscreen colour) are saved in the Pcbcraft design and change only the display-only layers; the saved `.solidcraft` model is the same for every choice. Each native body (laminate, copper, package envelopes) stores a Solidcraft appearance (`appearance.assign` with `pbr: {metallic, roughness, dielectric_ior}`). The viewport reads that stored look back, so it shows exactly what the file holds. Display-only layers (soldermask and via tents, plated finish, open-via bores, silkscreen, edge stackup bands, terminations, overcoat, solder) take their look from the same table; they are not saved (see below). The values live in `crates/solidcraft-integration/src/material.rs`.

## Conventions

- Colours are linear-sRGB reflectance (0–1), never gamma-encoded. In the native file they are stored as 8-bit sRGB, which round-trips to within one 8-bit step.
- Roughness is perceptual roughness (glTF/Burley), never below Solidcraft's 0.045.
- Dielectrics: F0 = ((n − 1)/(n + 1))². n = 1.52 gives 0.043; n = 1.55 gives 0.047.
- Metals: the base colour is the normal-incidence reflectance F0 per channel, and metallic = 1. Solidcraft still requires an IOR in [1, 3] for metals, so 1.5 is stored; it has no effect.
- Values marked **est.** are first-principles estimates, not measured spectra.

## Values

| Body kind | Look | Base colour (linear) | Metallic | Rough | IOR | Source |
|---|---|---|---|---|---|---|
| `copper_track`, `pad_copper` | Bare copper | (0.955, 0.638, 0.538) | 1 | 0.30 | – | Cu F0 from Johnson & Christy 1972 (Hoffman's table). Exposed only when the mask is hidden in the Layers popover; under the mask it is covered by the mask cap. |
| `pad_finish` | Per pad finish | see [pad finishes](#pad-finishes) | 1 | 0.16–0.35 | – | The board's pad finish; ENIG by default. |
| `termination` | Matte tin | (0.78, 0.78, 0.80) | 1 | 0.28 | – | **est.** Plated tin near polished Sn (≈ 0.75–0.8 across the visible); a satin finish. Raised from 0.64 / 0.28 after the M1–M3 visual review found terminations reading as grey plastic; roughness lowered from 0.40 to 0.28 in the founder's live look review (1 Nov 2026), because 0.40 blurred every reflection into flat grey. |
| `solder` | Reflowed solder fillet | (0.78, 0.78, 0.80) | 1 | 0.38 | – | **est.** SAC alloys are about 96 % Sn, so the Sn F0. Lead-free joints freeze satin; at 0.14 the fillet mirrored the studio's dark walls and read as black chrome. |
| `chip_resistor` | Alumina ceramic | (0.78, 0.77, 0.73) | 0 | 0.55 | 1.77 | **est.** 96 % alumina thick-film substrates reflect 0.75–0.85 across 400–700 nm (published substrate data); n from sapphire (n_o = 1.77, handbook). |
| `chip_capacitor` | MLCC ceramic | (0.32, 0.25, 0.15) | 0 | 0.55 | 2.4 | **est.** Sintered barium titanate, tan; n ≈ 2.4 (handbook). |
| `overcoat` | Resistor overcoat | (0.025, 0.025, 0.027) | 0 | 0.30 | 1.55 | **est.** Glass or epoxy overcoat with a black marking layer over the resistive film. |
| `substrate` | FR-4 laminate (board edge) | (0.36, 0.32, 0.17) | 0 | 0.60 | 1.55 | **est.** E-glass (n ≈ 1.55) in epoxy (n ≈ 1.55–1.6), CRC/Palik indices. The colour is the natural laminate's pale tan. |
| `mask_top`, `mask_bottom` | Soldermask over laminate | `mask_over(pigment, FR-4, 20 µm)`; green ≈ (0.0118, 0.116, 0.032) | 0 | per finish | 1.52 | Layer model and [mask colours](#mask-colours) below. Thickness and gloss come from LPI datasheets. |
| `mask_cap` | Soldermask over copper | `mask_over(pigment, Cu, 12 µm)`; green ≈ (0.064, 0.237, 0.069) | 0 | per finish | 1.52 | Lighter in every channel than over laminate, so traces read as lighter lines. |
| `package_body`, `part_body` | Black epoxy mould compound | (0.022, 0.022, 0.024) | 0 | 0.45 | 1.55 | **est.** Silica-filled epoxy (filler n ≈ 1.46, resin ≈ 1.57) with carbon black. EDM-textured top. |

The gloss mask roughness of 0.20 is **est.** from the LPI gloss texture code ("L"). Screenshots tuned it: 0.12 gave pin-point highlights under the analytic lights, and 0.28 washed the board out.

## Soldermask layer model

Solidcraft's opaque surface has no layered BSDF or transmission, so the mask is a single dielectric surface. Its gloss comes from the IOR and roughness. Its albedo is a precomputed composite of the pigmented layer over what lies beneath it. The composite is a two-flux layer with Saunderson's interface correction:

```
ρ(R, d) = ρ_scatter + (1 − F0)(1 − ρ_i) · T² R / (1 − ρ_i R)
T = exp(−σ d),  σ = −ln(T₂₀) / 20 µm
```

- `R`: diffuse reflectance under the mask (FR-4 albedo, or copper F0, which overestimates the micro-etched copper slightly).
- `d`: mask thickness, 20 µm over laminate and 12 µm over a track's crown (LPI thins over copper crowns and edges; 10–20 µm over copper in the LPI datasheets).
- `T₂₀`: **est.** per-pass transmittance through 20 µm of the pigmented resin. For green, (0.10, 0.45, 0.18): phthalocyanine green plus a yellow pigment pass 500–570 nm and absorb red and blue. Other colours are in [mask colours](#mask-colours).
- `ρ_scatter`: **est.** pigment back-scatter over a black base; green (0.010, 0.085, 0.030).
- `F0 = 0.043`: from n = 1.52.
- `ρ_i = 0.60`: internal diffuse reflectance of the mask/air interface for n ≈ 1.52 (first principles; Saunderson's k₂).

The laminate result lands close to the check target estimated independently from T²·R, ≈ (0.012, 0.10, 0.035). Over copper the 15 µm target was ≈ (0.02, 0.17, 0.04); the thinner 12 µm crown is lighter, so traces stand out more. Tests check that every albedo lies in [0, 1], that green dominates, and that the mask over copper is lighter in every channel.

## Geometry that carries the looks

The saved native model holds only physical bodies: the laminate, exact copper (pads, tracks on every copper layer and board vias), package envelopes and generated package models (one shared definition per package revision, every body with its material). They are disjoint except where copper joins copper (a track ending in its pad), so Solidcraft's interference checks and mass properties of the saved file are physical; a test checks the bodies' boxes pairwise and the total volume.

The stackup (`Project::stackup`) sets the heights: the laminate is the sum of the dielectrics (parameters `dielectric_N_thickness`, summed as `board_thickness`), each foil has its own thickness parameter (`copper_thickness` for the top; a foil as thick follows it), and inner copper sits at the top of the dielectric below it. Inner copper is embedded in the laminate, which is built as stacked slabs: a slab per dielectric with every drill as a hole, and at each inner foil's level a slab whose holes are also that layer's copper (tracks and via rings), where the copper is now; a test checks the laminate's volume against the exact areas. A via size is one component placed at each via of that size: a plated barrel (drill less 0.025 mm plating) through the laminate and an annular ring on each layer it joins, outer rings capping the barrel and inner rings round it (exact circles, the laminate's holes).

Everything else is a display-only layer built on the model worker as meshes in Solidcraft's mesh type (`crates/solidcraft-integration/src/appearance.rs`). They are drawn and picked like native bodies (mask caps and finish strips pick as their track, terminations, overcoat and solder as their part) but are never saved, and they are not native features, so they add nothing to native construction time.

**Body style.** Which body material a package gets comes only from its authored `PackageDefinition.form` (a generic form is moulded; a chip form's `kind` is resistor, capacitor or inductor; set in the Component panel's 3D tab, Body row) and, for generated models, its `model` materials ([package materials](#package-materials)). The 3D view never infers it from a designator, name, value or package size.

| Layer | Size | Why |
|---|---|---|
| Soldermask slabs | 0.020 mm over each board face, 0.03 mm in from the board outline | Thinner than real LPI over laminate (25–35 µm), so exposed pads stand clear of the depth bias at grazing angles. The pull-back (**est.**; fabricators pull mask back from a routed edge) keeps the mask's walls off the laminate's, which drew a dashed line along the far edge at the silhouette. |
| Mask cap over a track | 0.015 mm above the copper, 0.01 mm wider each side, round free ends | Round ends follow routed copper; the cap is interrupted wherever the track crosses a same-layer mask opening, including tracks passing through a land. |
| Mask opening | Land + 0.05 mm; an open via's ring + 0.05 mm | A common fabrication default. |
| Tented via | A dome of revolution over the ring: as high as a mask cap (copper + 0.015 mm), sagging 0.008 mm over the drill, sloping into the flat mask 0.08 mm beyond the ring | **est.** Dry-film and LPI tents dip a little into the hole they bridge; the slope keeps the bump from reading as a stamped disc. |
| Open via | Finished ring (as a land) and a dark sleeve 0.01 mm thick lining the bore, through the board and both foils (`via_bore`) | The view computes no occlusion, so a bore five or more diameters deep would light up like an open copper face; real ones read as dark holes. Reflectance 0.01 (**est.**). |
| Silkscreen | Legend ink 0.015 mm thick, half sunk into the mask; on a track's mask cap or a tent's crown it stands on the crown instead; clipped 0.05 mm clear of mask openings | Screen-printed and inkjet legend is 10–25 µm (**est.**); quoted silkscreen-to-pad clearances are 0.05–0.15 mm. Text uses Pcbcraft's own stroke font (`silkscreen/font.rs`) laid out by the core's text metric; bottom-side text is reflected in its box so it reads from below. |
| Edge stackup bands | Thin frames 0.002 mm proud of the routed faces (copper lines 0.003 mm): a band per dielectric (split around inner foils), a line per foil, a mask lip over each outer foil | Display only, so the board reads as a sectioned real board; the native laminate stays one body. Weave strength per glass style (**est.**): 7628 1.0, 2116 0.8, 1080 0.6, 106 0.45; prepreg 0.75 × its style. |
| Plated finish | 0.005 mm around each land; over track copper inside an opening, 0.004 mm | ENIG nickel is 3–6 µm (IPC-4552). Fabrication plates all copper a mask opening exposes, so no bare copper shows where a track enters a land. |
| Chip terminations | Both ends of the long axis, a band of a fifth of the length (at most 0.5 mm), 0.01 mm proud of the top, sides, end and underside | Chip parts are packages with a chip form (publication requires exactly two package terminals for these). On an envelope chip these are display-only, and an inductor shows the ceramic chip's terminations; a generated chip has real tin termination bodies and a ferrite inductor body. The bands follow EIA chip-resistor bands (0.3–0.5 mm from 0603 to 1206). The rule uses only the package, so the Component preview and the board agree. |
| Lead bands | Land ∩ body, 15 % of the body height | Moulded packages with at most two lands. |
| Resistor overcoat | Body top between the bands, 0.04 mm in from the long edges, 0.005 mm proud | Leaves the white alumina rim and sides visible. Only chip resistors get it; chip capacitors are MLCC ceramic without an overcoat. |
| Solder fillet | A closed solid: concave meniscus z = h(1 − s/reach)² from the termination face to the land's toe, rounding down to the land at both sides as 1 − u⁴ across 92 % of the termination width; h = 0.4 × body height (at most 0.4 mm) | J-STD-001 asks for at least a quarter of the termination height. Only where the end face stands over its land. Also drawn against a generated chip's real terminations while its model frame is the package frame (no datum shift or turn). No vertical side walls: a flat metal wall mirrored the studio's dark lower walls and read as a black hollow. |

Every display layer carries the copper layer it lies on, or the side a mask or legend film covers, so the 3D view's layer toggles can show and hide it.

## Package materials

Generated and envelope package models name a physical material per body (`MaterialId`), never a colour. The table is core data (`crates/editor-core/src/materials.rs`, `materials::builtin`), revisioned with the app (`BUILTIN_REVISION`); a project stores only the ids, and the saved `.solidcraft` model carries each body's opaque projection, so a project reopens with the same native looks even after a later table revision. User materials, once they exist, will be a revisioned library definition kind snapshotted like other definitions. The integration crate keeps its `BodyKind` looks (above) and maps body kinds to these ids; the values shared with the table above are the same numbers, and a core test (`materials::tests::cited_values_match_docs`) checks them.

Every value carries a source key: `johnson-christy-1972`, `rakic-1998`, `hoffman`, `palik-crc` (see [sources](#sources)) or `est`. A material with any `est` source is flagged `estimated`. Metals are metallic 1 with IOR 1.5 (ignored). Densities are handbook values for the elements, alumina, barium titanate and borosilicate glass, and typical grade values (**est.**) for compounds and plastics.

| Material | Base colour (linear) | Metallic | Rough | IOR | Extra | Density kg/m³ | Source |
|---|---|---|---|---|---|---|---|
| Epoxy mould compound | (0.022, 0.022, 0.024) | 0 | 0.45 | 1.55 | EDM texture 1.0 | 1950 | **est.** As `package_body` above. |
| Laser-marked epoxy | (0.10, 0.10, 0.105) | 0 | 0.75 | 1.55 | – | 1950 | **est.** Ablation exposes lighter silica filler and roughens the surface. |
| White marking ink | (0.80, 0.80, 0.78) | 0 | 0.60 | 1.50 | – | – | **est.** TiO₂-pigmented ink, as legend ink. |
| Resistor overcoat | (0.025, 0.025, 0.027) | 0 | 0.30 | 1.55 | – | – | **est.** As `overcoat` above. |
| 96 % alumina | (0.78, 0.77, 0.73) | 0 | 0.55 | 1.77 | – | 3720 | **est.** colour as `chip_resistor` above; n from sapphire. |
| Barium titanate | (0.32, 0.25, 0.15) | 0 | 0.55 | 2.4 | – | 6020 | **est.** colour as `chip_capacitor`; n and density handbook. |
| Ferrite | (0.045, 0.045, 0.047) | 0 | 0.60 | 2.0 | – | 4800 | **est.** Dark grey sintered Mn-Zn/Ni-Zn ferrite. |
| Matte tin | (0.78, 0.78, 0.80) | 1 | 0.32 | – | – | 7265 | **est.** F0 as `termination`; β-tin density; roughness 0.32: 0.40 went flat grey, and the 0.28 of the founder's look review mirrored the studio on chip terminations (realism review, v4). Chip terminations and exposed pads use it. |
| Bright tin | (0.78, 0.78, 0.80) | 1 | 0.24 | – | – | 7265 | **est.** A smoother brightened deposit, satin rather than mirror (0.22, then 0.15 in the founder's look review; 0.24 in v4). Gull-wing leads use it, so formed leads read as bright tin, not charcoal. |
| SAC solder | (0.78, 0.78, 0.80) | 1 | 0.38 | – | – | 7400 | **est.** As `solder` above. |
| Gold plating | (1.000, 0.782, 0.344) | 1 | 0.30 | – | – | 19300 | Au F0, Johnson & Christy via Hoffman; roughness **est.** Hard gold is thick enough that the nickel does not show (unlike ENIG pads). 0.30 (was 0.22): a square header pin then gathers the studio's strips into a visible specular gradient; smoother pins mirrored the dark walls and read as olive brass. |
| Nickel | (0.660, 0.609, 0.526) | 1 | 0.30 | – | – | 8908 | Ni F0, Rakić 1998; roughness **est.** |
| Copper-alloy leadframe | (0.955, 0.638, 0.538) | 1 | 0.32 | – | – | 8910 | Cu F0 (C194-like alloys are over 97 % Cu); roughness **est.** |
| Phosphor bronze | (0.90, 0.70, 0.52) | 1 | 0.30 | – | – | 8860 | **est.** Cu with about 5 % Sn, shifted toward tin. |
| Brass | (0.910, 0.778, 0.423) | 1 | 0.28 | – | – | 8500 | Brass F0, Hoffman's table; roughness **est.** |
| Aluminium | (0.913, 0.922, 0.924) | 1 | 0.30 | – | – | 2700 | Al F0, Hoffman's table; roughness **est.** |
| PVC sleeve | (0.020, 0.045, 0.16) | 0 | 0.35 | 1.54 | – | 1380 | **est.** colour of a blue sleeve; n handbook. |
| Borosilicate glass | (0.85, 0.88, 0.86) | 0 | 0.05 | 1.47 | transmission 1.0, (0.92, 0.95, 0.93) at 1 mm | 2230 | n and density handbook; attenuation **est.** Saved as an opaque tint until a layered saved appearance exists. |
| PBT | (0.030, 0.030, 0.032) | 0 | 0.50 | 1.57 | – | 1530 | **est.** Black 30 % glass-filled housing. |
| LCP | (0.025, 0.025, 0.028) | 0 | 0.35 | 1.60 | – | 1620 | **est.** Black glass-filled housing. |
| PA9T | (0.030, 0.030, 0.030) | 0 | 0.45 | 1.53 | – | 1450 | **est.** Black glass-filled housing. |
| Clear silicone | (0.90, 0.90, 0.90) | 0 | 0.08 | 1.41 | transmission 1.0, (0.97, 0.97, 0.96) at 1 mm | 1030 | n of PDMS handbook; attenuation **est.** |
| Diffuse silicone | (0.75, 0.75, 0.72) | 0 | 0.40 | 1.41 | transmission 0.3, (0.80, 0.80, 0.78) at 0.5 mm | 1100 | **est.** Scattering filler. |
| Polyimide | (0.40, 0.15, 0.02) | 0 | 0.25 | 1.70 | – | 1420 | n and density handbook; amber colour **est.** |

Family defaults (`library::default_material`, never from names): chip bodies alumina (resistor), barium titanate (capacitor) or ferrite (inductor); header housings PBT and pins gold; leads, terminations and exposed pads matte tin; markings laser-marked epoxy; lenses clear silicone; sleeves PVC; every other body epoxy mould compound. A generated model may override a role (`GeneratedMaterials`).

## Fabrication presets

The board's fabrication attributes are typed data in the Pcbcraft design (`Project::fabrication`), set with `layout.set_fabrication` from the PCB Board card's Board setup dialog (Soldermask, Mask finish, Pad finish, Silkscreen), the CLI or MCP. Defaults: green, gloss, ENIG, white legend. Every combination is valid data; whether a fabricator offers it is theirs to say. The 3D view draws the mask as a lacquer: a clear coat (weight 1, IOR 1.52, the [finish](#mask-finish)'s roughness) carries the gloss over a diffuse pigmented body, drawn with Solidcraft's opt-in product look ([PCB 3D look](#pcb-3d-look)). The "over laminate/over copper" values below are the layer's whole diffuse reflectance (`mask_over`); the body under the coat is that divided by (1 − F0) = 0.957 (`mask_body`), because the coat applies the air-to-resin Fresnel transmission. The body has IOR 1 (no interface of its own) and roughness 1. Solidcraft's split-sum studio light takes the coat's reflectance out of the body once, so the mask's total diffuse reflectance under the studio matches the table; its direct lights attenuate at entry and exit, about 4 % darker (**est.** from F0).

### Mask colours

Every value in this table is **est.**: no soldermask colour standard exists, and hue varies by batch and supplier ([NCAB](https://www.ncabgroup.com/blog/pcb-solder-mask/)). T₂₀ and scatter were chosen so that the layer model reproduces the qualitative descriptions below, then checked by luminance ratio. "Show-through" is the Rec. 709 luminance of the mask over copper divided by that over laminate, a measure of how strongly traces read.

| Colour | T₂₀ | ρ_scatter | Over laminate (linear) | Over copper (linear) | Show-through |
|---|---|---|---|---|---|
| Green | (0.10, 0.45, 0.18) | (0.010, 0.085, 0.030) | (0.012, 0.116, 0.032) | (0.064, 0.237, 0.069) | 2.1× |
| Blue | (0.03, 0.10, 0.35) | (0.006, 0.025, 0.110) | (0.006, 0.027, 0.119) | (0.019, 0.050, 0.196) | 1.9× |
| Red | (0.30, 0.03, 0.03) | (0.280, 0.014, 0.012) | (0.296, 0.014, 0.012) | (0.482, 0.020, 0.017) | 1.6× |
| Purple | (0.14, 0.035, 0.22) | (0.074, 0.008, 0.102) | (0.077, 0.008, 0.106) | (0.155, 0.015, 0.151) | 1.8× |
| Yellow | (0.30, 0.26, 0.03) | (0.600, 0.440, 0.022) | (0.616, 0.450, 0.022) | (0.802, 0.519, 0.027) | 1.2× |
| Black | (0.011, 0.012, 0.015) | (0.024, 0.024, 0.026) | (0.024, 0.024, 0.026) | (0.028, 0.026, 0.028) | 1.1× |
| White | (0.08, 0.087, 0.06) | (0.70, 0.70, 0.66) | (0.701, 0.701, 0.660) | (0.741, 0.721, 0.670) | 1.03× |
| Clear | (0.92, 0.88, 0.87) | (0.005, 0.011, 0.002) | (0.154, 0.128, 0.057) | (0.780, 0.350, 0.259) | 3.4× |

Calibration notes:

- **Brightness (second pass, 30 October 2026).** A visual check of the product-look pictures found red a dark maroon, yellow a dark mustard and black crushed to pure black, against the scarlet, lemon-ochre and dark-grey gloss boards the cited articles show. None of the cited sources gives a measured colour, so the values stay **est.**; red and yellow now carry more pigment scatter (red also more transmission, so traces still read at 1.6×), and black scatters about 2.4 % (L* ≈ 17), a dark grey under its lacquer rather than a void. The show-through order below is unchanged.

- **Opacity order.** Taiyo's minimum exposure energy is a rough proxy for how much a pigment blocks light: black, white and yellow ≥ 800 mJ/cm²; blue and red ≥ 300; clear ≥ 200 ([Taiyo PSR-4000 BN DI colours](https://sc-c.digikeyassets.com/-/media/Images/Marketing/Resources/DKRed/TDS_PSR-4000_BN_DI_Colors_October-6-2020.pdf)). Peters calls its green "green transparent" ([Elpemer 2467](https://www.wedirekt.com/web/cms/content/_files//pdf1/technical_report_elpemer_2467.pdf)).
- **Trace contrast.** Observers report traces nearly invisible under black, least visible under white, murky under yellow, and noticeably less visible under red than under green ([Andy Brown](https://andybrown.me.uk/?p=4010), [JLCPCB](https://jlcpcb.com/blog/pcb-solder-mask-colors-performance)). A first estimate gave red 2.1× and purple 2.3×, at or above green, which contradicts that; both were recalibrated with more pigment scatter and less transmission (1.7× and 1.8×; red is now 1.6× after the brightness pass below). The first yellow estimate put the mask over copper at 0.77 red, too bright for a murky mask; it was lowered to 0.62 (1.3×), which the brightness pass found too dark, so it is now 0.80 with traces barely lightening it (1.2×). Tests hold this order: clear above 3×, red, purple and blue at least 0.2 below green (blue's place is **est.**: no cited observation compares blue with green; it follows from blue's opacity class, which Taiyo groups with red), yellow and black below 1.4×, white below 1.1×.
- **Clear** assumes natural FR-4 under it. Black FR-4 (as in clear-over-black boards) would need its own laminate reflectance.
- **White** is scatter-dominated, where the two-flux layer model is crude.
- Colours offered: JLCPCB lists green, blue, red, black, white, purple and yellow; PCBWay adds matte red and blue, orange, grey, pink and transparent ([circuitstate](https://circuitstate.com/?p=6823)). Orange, grey and pink are not offered here yet.

### Mask finish

Measured gloss masks read 80–94 and matte masks 3–43 on a handheld glossmeter, probably 60° gloss units ([NPL MAT 5](https://eprintspublications.npl.co.uk/4043/1/MAT5.pdf)). The finish sets the lacquer coat's perceptual roughness; the mapping is **est.** A single-surface look needed a compromise (0.12 too pin-point, 0.28 washed out); with the coat carrying the gloss and the body diffuse, a gloss coat can be sharp without bleaching the colour.

| Finish | Coat roughness | Exceptions |
|---|---|---|
| Gloss | 0.06 | Clear 0.05 (no pigment breaks the surface); yellow 0.12 (fabricators sell it semi-gloss) |
| Satin | 0.20 | – |
| Matte | 0.40 | Black 0.45 |

### Surface patterns

Drawn by the product look from the world position in millimetres (display only). Glass style 7628 (44 × 31 ends per inch, 0.173 mm thick; [Isola glass fabric](https://www.isola-group.com/wp-content/uploads/Isola-Glass-Fabric-04_2022.pdf)) gives a yarn pitch of 0.577 × 0.819 mm and the ply period up a routed edge. Pattern amplitudes are Solidcraft's (visual estimates). Pcbcraft chooses where they show and how strongly:

| Body | Pattern | Strength |
|---|---|---|
| FR-4 laminate (routed edge, bare laminate) | Weave (colour and relief); a routed edge shows it at a quarter of a face's swing | 1 |
| Mask over laminate | Orange peel, plus the weave in colour only | Weave show-through: clear 0.6, green 0.12, purple 0.10, blue and red 0.08; black, white and yellow none (peel only) |
| Mask over copper | Orange peel | 1 |

The weave is a plain weave of continuous warp and weft bundles that rise over one crossing and dip under the next, with filament striations along each bundle; only the windows between four crossings are gaps, so it reads as cloth, not tiles. It and its striations fade out by the time their period falls to about three pixels, so there is no moiré at board distance. Orange peel is a long, shallow waviness (2.4 and 0.9 mm wavelengths, 6 and 1 µm relief, slopes of a few milliradians) so a gloss mask's softbox reflection keeps its shape; all relief fades out toward grazing views. These amplitudes are Solidcraft's visual estimates, lowered after a visual check found the earlier peel turned the softbox into ripples and the weave into a brick-like grid.

Show-through is **est.** No measured print-through value was found in public sources; the mask levels the surface and the weave changes the colour beneath. It follows the opacity order: clear resin shows the weave plainly (as clear-mask boards do), the translucent pigments faintly, the most opaque ones (black, white, yellow: ≥ 800 mJ/cm² exposure, Taiyo) not at all. Legend ink shows the silkscreen grain; the edge bands show the weave per glass style.

### Pad finishes

| Pad finish | Base colour (linear F0) | Rough | Source |
|---|---|---|---|
| ENIG (default) | (1.0, 0.73, 0.30) | 0.28 | 85 % Au F0 (1.000, 0.782, 0.344; J&C/Hoffman) blended with 15 % Ni F0 (0.660, 0.609, 0.526; Rakić 1998). IPC-4552 gold is 0.04–0.10 µm, only 1–2 optical penetration depths. The electroless Ni-P is nodular, so the finish is satin (**est.** roughness, raised from 0.25 so a typical view averages more of the studio and reads as pale gold, not brass). The M1–M3 visual review found the blend reading as desaturated khaki, so the F0 was moved to the gold F0 with a little nickel showing (1.0, 0.73, 0.30) at 0.28 (**est.**). |
| HASL (tin-lead) | (0.62, 0.62, 0.63) | 0.16 | **est.** Eutectic Sn63Pb37 is about 30 % lead by volume; lead reflects less than tin, so the blend sits a little below tin. Eutectic solder freezes bright. To be confirmed against Palik's Sn and Pb n, k. |
| Lead-free HASL | (0.64, 0.64, 0.66) | 0.20 | **est.** Lead-free solders (SAC, SnCuNi) are over 95 % tin. Polished tin reflects ≈ 0.7, flat across the visible; a reflowed or plated surface carries an oxide film, so it is taken lower. They freeze duller and grainier than eutectic tin-lead, hence the higher roughness. |
| OSP | (0.955, 0.638, 0.538) | 0.35 | Bare copper F0 (as `copper_track`). The organic film is sub-micrometre and transparent (**est.**: its thin-film tint is ignored); the copper under it is micro-etched, hence the roughness. |
| Immersion silver | (0.972, 0.960, 0.915) | 0.20 | Ag F0 from Hoffman's table (measured optical constants). Immersion silver (IPC-4553) is many optical penetration depths thick, so the copper does not show. |

### Silkscreen colour

White (default), black or yellow: matte epoxy legend ink (roughness 0.75; reflectance **est.** white 0.78, black 0.03, yellow (0.70, 0.52, 0.04)) with the silkscreen grain at half strength (**est.**; full strength read as blotchy dirt at mid zoom), on the sides the stackup prints.

## PCB 3D look

The PCB 3D view and Component 3D preview draw with Solidcraft's opt-in product look (`Frame::product_look`): a procedural studio environment (the high-contrast Contrast preset: strip lights over a grey cyclorama, a dark floor), fixed in the world and turned 145° about +Z, so one strip light's mirror image is a crisp-edged lacquer band across the overview's far end (a quarter turn read flat, the unturned studio washed a third of the board pale with a blurred edge; realism review v4); Khronos PBR Neutral tone mapping at exposure 1.1; a dithered sRGB encode; clear coats; surface patterns; and ambient occlusion on the studio's light, radius 2 % of the model's diagonal within 0.15–1.2 mm (about a chip's height on a board). Over it, a key (point), fill and rim light turn with the camera, the rim grazing in at 12° so its widened lobe's mirror image falls past the board's far edge instead of glowing across close-ups (it sat at 35° before realism review v4), and four floor-bounce lights 25° below the horizon stay in the world, so vertical metal (termination ends, fillets) does not mirror the studio's dark lower walls. Seen from below, the board faces the dark floor, so the direct lights are 2.5× stronger and an ambient light (0.6) is added. The look treats each direct light as a small softbox on a clear coat (Solidcraft widens the coat's lobe for it), so the key light leaves a soft highlight on gloss mask rather than a clipped dot. All of these are tuned by screenshot (**est.**); pictures are written by `cargo test --profile ci -p pcbcraft write_pictures -- --ignored` (and `write_showcase_pictures` for the showcase set: overview, SOIC, QFN, header and passive close-ups, tented and open vias, the edge stackup and the bottom side) with `PCBCRAFT_PICTURES` set. `the_overview_shows_a_sheen_on_the_mask_without_clipping` measures the overview on a GPU: the 95th-percentile mask luminance is at least twice the median (3.8× measured at 145°; 1.1× at 140°, where the band leaves the board, and 1.6× with the earlier quarter turn) and no pixel clips.

Generated package models have rounded edges so they catch highlights instead of ending in razor edges (radii **est.**, each a named model parameter that drives a native fillet and is capped by the form): moulded gull-wing bodies 0.2 mm upright corners and 0.12 mm top edges, no-lead bodies 0.06 mm top edges (sawn mouldings stay nearly sharp), header bases 0.3 mm corners and 0.2 mm top edges, chip bodies 0.06 mm long edges (at most the overcoat inset on a resistor) and each termination's outer end 0.08 mm.

Tin and tin-lead were lowered (from 0.70 and 0.68) and made smoother (HASL from 0.20, solder from 0.15, terminations from 0.35) after the visual check found HASL pads and fillets reading as flat white paper: at a high F0 and moderate roughness the studio's average saturated to a uniform white, with no reflection detail. Both changes are **est.**

## Sources

The core table's source keys are `johnson-christy-1972`, `rakic-1998`, `hoffman` and `palik-crc` (Palik and the CRC Handbook), the first four entries below; `est` marks an estimate.

- P. B. Johnson and R. W. Christy, "Optical Constants of the Noble Metals", *Phys. Rev. B* 6, 4370 (1972). Au and Cu; tabulated at refractiveindex.info.
- A. D. Rakić et al., "Optical properties of metallic films for vertical-cavity optoelectronic devices", *Applied Optics* 37, 5271 (1998). Ni.
- N. Hoffman, "Physics and Math of Shading", SIGGRAPH Physically Based Shading course notes (2010/2013). The linear-RGB metal F0 method and table.
- E. D. Palik, *Handbook of Optical Constants of Solids*, and the CRC Handbook of Chemistry and Physics. Refractive indices of epoxy, E-glass and silica; Sn, to confirm the tin estimate.
- B. Burley, "Physically-Based Shading at Disney" (2012). Perceptual roughness.
- IPC-4552, ENIG: Ni 3–6 µm, Au 0.04–0.10 µm. IPC-4553, immersion silver.
- Soldermask colour and gloss: NPL MAT 5 (glossmeter readings and thickness), Taiyo PSR-4000 BN DI colour datasheet (exposure energy), Peters Elpemer 2467 report, NCAB, Andy Brown, JLCPCB and circuitstate articles; links in [mask colours](#mask-colours). Public articles and datasheets only.
- Isola, *Glass Fabric* data sheet (04/2022): style 7628 counts and thickness.
- Clear-coat layering after the OpenPBR coat model (Academy Software Foundation specification); the shader is Solidcraft's.
- Manufacturer LPI soldermask datasheets, thickness and gloss/matte texture codes only: Florida CirTech LPI-950 LC-1, LPI-950 C-8GD and LPI-950 S-22.

No EDA tool, EDA library, 3D model or material file was consulted.

## Planned confirmation

The pigment values (mask T and scatter for every colour, FR-4 tint, mould compound), the gloss-to-roughness mapping and the tin and tin-lead F0 are estimates. The planned confirmation is a photograph of one of our own boards next to an X-Rite/Calibrite colour chart, under known lighting. Fit only those parameters and keep the physical model.
