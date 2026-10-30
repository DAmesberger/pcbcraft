# Physical looks of the PCB 3D model

The PCB 3D view and the saved `.solidcraft` model use the same physical looks. The board's [fabrication attributes](#fabrication-presets) (mask colour and finish, pad finish, silkscreen colour) are saved in the Pcbcraft design and change only the display-only layers; the saved `.solidcraft` model is the same for every choice. Each native body (laminate, copper, package envelopes) stores a Solidcraft appearance (`appearance.assign` with `pbr: {metallic, roughness, dielectric_ior}`). The viewport reads that stored look back, so it shows exactly what the file holds. Display-only layers (soldermask, plated finish, terminations, overcoat, solder) take their look from the same table; they are not saved (see below). The values live in `crates/solidcraft-integration/src/material.rs`.

## Conventions

- Colours are linear-sRGB reflectance (0–1), never gamma-encoded. In the native file they are stored as 8-bit sRGB, which round-trips to within one 8-bit step.
- Roughness is perceptual roughness (glTF/Burley), never below Solidcraft's 0.045.
- Dielectrics: F0 = ((n − 1)/(n + 1))². n = 1.52 gives 0.043; n = 1.55 gives 0.047.
- Metals: the base colour is the normal-incidence reflectance F0 per channel, and metallic = 1. Solidcraft still requires an IOR in [1, 3] for metals, so 1.5 is stored; it has no effect.
- Values marked **est.** are first-principles estimates, not measured spectra.

## Values

| Body kind | Look | Base colour (linear) | Metallic | Rough | IOR | Source |
|---|---|---|---|---|---|---|
| `copper_track`, `pad_copper` | Bare copper | (0.955, 0.638, 0.538) | 1 | 0.30 | – | Cu F0 from Johnson & Christy 1972 (Hoffman's table). Exposed only with Hide substrate; under the mask it is covered by the mask cap. |
| `pad_finish` | Per pad finish | see [pad finishes](#pad-finishes) | 1 | 0.20–0.35 | – | The board's pad finish; ENIG by default. |
| `termination` | Matte tin | (0.70, 0.70, 0.72) | 1 | 0.35 | – | **est.** Same Sn F0 as HASL. Electroplated matte tin is rougher than reflowed solder. |
| `solder` | Reflowed solder fillet | (0.70, 0.70, 0.72) | 1 | 0.15 | – | **est.** SAC alloys are about 96 % Sn, so the Sn F0. A reflowed meniscus freezes smooth and bright. |
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

The saved native model holds only physical bodies: the laminate, exact copper (pads and tracks) and package envelopes. They are disjoint except where copper joins copper (a track ending in its pad), so Solidcraft's interference checks and mass properties of the saved file are physical; a test checks the bodies' boxes pairwise and the total volume.

Everything else is a display-only layer built on the model worker as meshes in Solidcraft's mesh type (`crates/solidcraft-integration/src/appearance.rs`). They are drawn and picked like native bodies (mask caps and finish strips pick as their track, terminations, overcoat and solder as their part) but are never saved, and they are not native features, so they add nothing to native construction time.

**Body style.** Which body material a package gets comes only from its authored `PackageDefinition.body` (`moulded`, `chip_resistor`, `chip_capacitor`; set in the Component panel's 3D tab, Body row). The 3D view never infers it from a designator, name, value or package size.

| Layer | Size | Why |
|---|---|---|
| Soldermask slabs | 0.020 mm over each board face | Thinner than real LPI over laminate (25–35 µm), so exposed pads stand clear of the depth bias at grazing angles. |
| Mask cap over a track | 0.015 mm above the copper, 0.01 mm wider each side, round free ends | Round ends follow routed copper; the cap is interrupted wherever the track crosses a same-layer mask opening, including tracks passing through a land. |
| Mask opening | Land + 0.05 mm | A common fabrication default. |
| Plated finish | 0.005 mm around each land; over track copper inside an opening, 0.004 mm | ENIG nickel is 3–6 µm (IPC-4552). Fabrication plates all copper a mask opening exposes, so no bare copper shows where a track enters a land. |
| Chip terminations | Both ends of the long axis, a band of a fifth of the length (at most 0.5 mm), 0.01 mm proud of the top, sides, end and underside | Chip parts are packages whose `body` is `chip_resistor` or `chip_capacitor` (publication requires exactly two package terminals for these). The bands follow EIA chip-resistor bands (0.3–0.5 mm from 0603 to 1206). The rule uses only the package, so the Component preview and the board agree. |
| Lead bands | Land ∩ body, 15 % of the body height | Moulded packages with at most two lands. |
| Resistor overcoat | Body top between the bands, 0.04 mm in from the long edges, 0.005 mm proud | Leaves the white alumina rim and sides visible. Only `chip_resistor` bodies get it; `chip_capacitor` bodies are MLCC ceramic without an overcoat. |
| Solder fillet | Concave meniscus z = h(1 − s/reach)² from the termination face to the land's toe; h = 0.4 × body height (at most 0.4 mm); 92 % of the termination width | J-STD-001 asks for at least a quarter of the termination height. Only where the end face stands over its land. |

There is no silkscreen: the core has no outline or reference-designator graphics yet. The silkscreen colour is saved with the design and will be drawn once legend graphics exist.

## Fabrication presets

The board's fabrication attributes are typed data in the Pcbcraft design (`Project::fabrication`), set with `board.set_fabrication` from the PCB properties card (Board: Soldermask, Mask finish, Pad finish, Silkscreen), the CLI or MCP. Defaults: green, gloss, ENIG, white legend. Every combination is valid data; whether a fabricator offers it is theirs to say. The looks are single-surface values: Solidcraft's real-time shader has no clear coat yet, so the gloss is a roughness, not a lacquer layer.

### Mask colours

Every value in this table is **est.**: no soldermask colour standard exists, and hue varies by batch and supplier ([NCAB](https://www.ncabgroup.com/blog/pcb-solder-mask/)). T₂₀ and scatter were chosen so that the layer model reproduces the qualitative descriptions below, then checked by luminance ratio. "Show-through" is the Rec. 709 luminance of the mask over copper divided by that over laminate, a measure of how strongly traces read.

| Colour | T₂₀ | ρ_scatter | Over laminate (linear) | Over copper (linear) | Show-through |
|---|---|---|---|---|---|
| Green | (0.10, 0.45, 0.18) | (0.010, 0.085, 0.030) | (0.012, 0.116, 0.032) | (0.064, 0.237, 0.069) | 2.1× |
| Blue | (0.03, 0.10, 0.35) | (0.006, 0.025, 0.110) | (0.006, 0.027, 0.119) | (0.019, 0.050, 0.196) | 1.9× |
| Red | (0.18, 0.03, 0.03) | (0.145, 0.008, 0.008) | (0.151, 0.008, 0.008) | (0.254, 0.014, 0.013) | 1.7× |
| Purple | (0.14, 0.035, 0.22) | (0.074, 0.008, 0.102) | (0.077, 0.008, 0.106) | (0.155, 0.015, 0.151) | 1.8× |
| Yellow | (0.30, 0.26, 0.03) | (0.420, 0.305, 0.020) | (0.436, 0.315, 0.020) | (0.622, 0.384, 0.025) | 1.3× |
| Black | (0.011, 0.012, 0.015) | (0.008, 0.008, 0.008) | (0.008, 0.008, 0.008) | (0.012, 0.010, 0.010) | 1.3× |
| White | (0.08, 0.087, 0.06) | (0.70, 0.70, 0.66) | (0.701, 0.701, 0.660) | (0.741, 0.721, 0.670) | 1.03× |
| Clear | (0.92, 0.88, 0.87) | (0.005, 0.011, 0.002) | (0.154, 0.128, 0.057) | (0.780, 0.350, 0.259) | 3.4× |

Calibration notes:

- **Opacity order.** Taiyo's minimum exposure energy is a rough proxy for how much a pigment blocks light: black, white and yellow ≥ 800 mJ/cm²; blue and red ≥ 300; clear ≥ 200 ([Taiyo PSR-4000 BN DI colours](https://sc-c.digikeyassets.com/-/media/Images/Marketing/Resources/DKRed/TDS_PSR-4000_BN_DI_Colors_October-6-2020.pdf)). Peters calls its green "green transparent" ([Elpemer 2467](https://www.wedirekt.com/web/cms/content/_files//pdf1/technical_report_elpemer_2467.pdf)).
- **Trace contrast.** Observers report traces nearly invisible under black, least visible under white, murky under yellow, and noticeably less visible under red than under green ([Andy Brown](https://andybrown.me.uk/?p=4010), [JLCPCB](https://jlcpcb.com/blog/pcb-solder-mask-colors-performance)). A first estimate gave red 2.1× and purple 2.3×, at or above green, which contradicts that; both were recalibrated with more pigment scatter and less transmission (now 1.7× and 1.8×). The first yellow estimate put the mask over copper at 0.77 red, too bright for a murky mask; it is now 0.62 (1.3×). Tests hold this order: clear above 3×, red, purple and blue at least 0.2 below green, yellow and black below 1.4×, white below 1.1×.
- **Clear** assumes natural FR-4 under it. Black FR-4 (as in clear-over-black boards) would need its own laminate reflectance.
- **White** is scatter-dominated, where the two-flux layer model is crude.
- Colours offered: JLCPCB lists green, blue, red, black, white, purple and yellow; PCBWay adds matte red and blue, orange, grey, pink and transparent ([circuitstate](https://circuitstate.com/?p=6823)). Orange, grey and pink are not offered here yet.

### Mask finish

Measured gloss masks read 80–94 and matte masks 3–43 on a handheld glossmeter, probably 60° gloss units ([NPL MAT 5](https://eprintspublications.npl.co.uk/4043/1/MAT5.pdf)). The mapping to perceptual roughness is **est.**:

| Finish | Roughness | Exceptions |
|---|---|---|
| Gloss | 0.20 | Clear 0.15 (no pigment breaks the surface); yellow 0.30 (fabricators sell it semi-gloss) |
| Satin | 0.35 | – |
| Matte | 0.55 | Black 0.60 |

### Pad finishes

| Pad finish | Base colour (linear F0) | Rough | Source |
|---|---|---|---|
| ENIG (default) | (0.95, 0.76, 0.37) | 0.25 | 85 % Au F0 (1.000, 0.782, 0.344; J&C/Hoffman) blended with 15 % Ni F0 (0.660, 0.609, 0.526; Rakić 1998). IPC-4552 gold is 0.04–0.10 µm, only 1–2 optical penetration depths. The electroless Ni-P is nodular, so the finish is satin. |
| HASL (tin-lead) | (0.68, 0.68, 0.69) | 0.20 | **est.** Eutectic Sn63Pb37 is about 30 % lead by volume; lead reflects less than tin, so the blend sits a little below tin. Eutectic solder freezes bright. To be confirmed against Palik's Sn and Pb n, k. |
| Lead-free HASL | (0.70, 0.70, 0.72) | 0.30 | **est.** Lead-free solders (SAC, SnCuNi) are over 95 % tin: polished tin reflects ≈ 0.7, flat across the visible. They freeze duller and grainier than eutectic tin-lead, hence the higher roughness. |
| OSP | (0.955, 0.638, 0.538) | 0.35 | Bare copper F0 (as `copper_track`). The organic film is sub-micrometre and transparent (**est.**: its thin-film tint is ignored); the copper under it is micro-etched, hence the roughness. |
| Immersion silver | (0.972, 0.960, 0.915) | 0.20 | Ag F0 from Hoffman's table (measured optical constants). Immersion silver (IPC-4553) is many optical penetration depths thick, so the copper does not show. |

### Silkscreen colour

White (default), black or yellow. Saved and exposed to CLI and MCP; not drawn until the board has legend graphics.

## Sources

- P. B. Johnson and R. W. Christy, "Optical Constants of the Noble Metals", *Phys. Rev. B* 6, 4370 (1972). Au and Cu; tabulated at refractiveindex.info.
- A. D. Rakić et al., "Optical properties of metallic films for vertical-cavity optoelectronic devices", *Applied Optics* 37, 5271 (1998). Ni.
- N. Hoffman, "Physics and Math of Shading", SIGGRAPH Physically Based Shading course notes (2010/2013). The linear-RGB metal F0 method and table.
- E. D. Palik, *Handbook of Optical Constants of Solids*, and the CRC Handbook of Chemistry and Physics. Refractive indices of epoxy, E-glass and silica; Sn, to confirm the tin estimate.
- B. Burley, "Physically-Based Shading at Disney" (2012). Perceptual roughness.
- IPC-4552, ENIG: Ni 3–6 µm, Au 0.04–0.10 µm. IPC-4553, immersion silver.
- Soldermask colour and gloss: NPL MAT 5 (glossmeter readings and thickness), Taiyo PSR-4000 BN DI colour datasheet (exposure energy), Peters Elpemer 2467 report, NCAB, Andy Brown, JLCPCB and circuitstate articles; links in [mask colours](#mask-colours). Public articles and datasheets only.
- Manufacturer LPI soldermask datasheets, thickness and gloss/matte texture codes only: Florida CirTech LPI-950 LC-1, LPI-950 C-8GD and LPI-950 S-22.

No EDA tool, EDA library, 3D model or material file was consulted.

## Planned confirmation

The pigment values (mask T and scatter for every colour, FR-4 tint, mould compound), the gloss-to-roughness mapping and the tin and tin-lead F0 are estimates. The planned confirmation is a photograph of one of our own boards next to an X-Rite/Calibrite colour chart, under known lighting. Fit only those parameters and keep the physical model.
