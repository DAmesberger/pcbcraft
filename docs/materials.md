# Physical looks of the PCB 3D model

The PCB 3D view and the saved `.solidcraft` model use the same physical looks. Each native body (laminate, copper, package envelopes) stores a Solidcraft appearance (`appearance.assign` with `pbr: {metallic, roughness, dielectric_ior}`). The viewport reads that stored look back, so it shows exactly what the file holds. Display-only layers (soldermask, plated finish, terminations, overcoat, solder) take their look from the same table; they are not saved (see below). The values live in `crates/solidcraft-integration/src/material.rs`.

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
| `pad_finish` (ENIG, default) | ENIG gold | (0.95, 0.76, 0.37) | 1 | 0.25 | – | 85 % Au F0 (1.000, 0.782, 0.344; J&C/Hoffman) blended with 15 % Ni F0 (0.660, 0.609, 0.526; Rakić 1998). IPC-4552 gold is 0.04–0.10 µm, only 1–2 optical penetration depths. The electroless Ni-P is nodular, so the finish is satin. |
| `pad_finish` (HASL) | Tin solder | (0.70, 0.70, 0.72) | 1 | 0.25 | – | **est.** Polished tin reflects ≈ 0.7, flat across the visible. To be confirmed against Palik's Sn n, k. |
| `termination` | Matte tin | (0.70, 0.70, 0.72) | 1 | 0.35 | – | **est.** Same Sn F0 as HASL. Electroplated matte tin is rougher than reflowed solder. |
| `solder` | Reflowed solder fillet | (0.70, 0.70, 0.72) | 1 | 0.15 | – | **est.** SAC alloys are about 96 % Sn, so the Sn F0. A reflowed meniscus freezes smooth and bright. |
| `chip_resistor` | Alumina ceramic | (0.78, 0.77, 0.73) | 0 | 0.55 | 1.77 | **est.** 96 % alumina thick-film substrates reflect 0.75–0.85 across 400–700 nm (published substrate data); n from sapphire (n_o = 1.77, handbook). |
| `chip_capacitor` | MLCC ceramic | (0.32, 0.25, 0.15) | 0 | 0.55 | 2.4 | **est.** Sintered barium titanate, tan; n ≈ 2.4 (handbook). |
| `overcoat` | Resistor overcoat | (0.025, 0.025, 0.027) | 0 | 0.30 | 1.55 | **est.** Glass or epoxy overcoat with a black marking layer over the resistive film. |
| `substrate` | FR-4 laminate (board edge) | (0.36, 0.32, 0.17) | 0 | 0.60 | 1.55 | **est.** E-glass (n ≈ 1.55) in epoxy (n ≈ 1.55–1.6), CRC/Palik indices. The colour is the natural laminate's pale tan. |
| `mask_top`, `mask_bottom` | Soldermask over laminate | `mask_over(FR-4, 20 µm)` ≈ (0.0118, 0.116, 0.032) | 0 | 0.20 | 1.52 | Layer model below. Thickness and gloss come from LPI datasheets. |
| `mask_cap` | Soldermask over copper | `mask_over(Cu, 12 µm)` ≈ (0.064, 0.237, 0.069) | 0 | 0.20 | 1.52 | Layer model below. Lighter in every channel than over laminate, so traces read as lighter green. |
| `package_body`, `part_body` | Black epoxy mould compound | (0.022, 0.022, 0.024) | 0 | 0.45 | 1.55 | **est.** Silica-filled epoxy (filler n ≈ 1.46, resin ≈ 1.57) with carbon black. EDM-textured top. |

The mask roughness of 0.20 is **est.** from the LPI gloss texture code ("L"). Screenshots tuned it: 0.12 gave pin-point highlights under the analytic lights, and 0.28 washed the board out.

## Soldermask layer model

Solidcraft's opaque surface has no layered BSDF or transmission, so the mask is a single dielectric surface. Its gloss comes from the IOR and roughness. Its albedo is a precomputed composite of the pigmented layer over what lies beneath it. The composite is a two-flux layer with Saunderson's interface correction:

```
ρ(R, d) = ρ_scatter + (1 − F0)(1 − ρ_i) · T² R / (1 − ρ_i R)
T = exp(−σ d),  σ = −ln(T₂₀) / 20 µm
```

- `R`: diffuse reflectance under the mask (FR-4 albedo, or copper F0, which overestimates the micro-etched copper slightly).
- `d`: mask thickness, 20 µm over laminate and 12 µm over a track's crown (LPI thins over copper crowns and edges; 10–20 µm over copper in the LPI datasheets).
- `T₂₀ = (0.10, 0.45, 0.18)`: **est.** per-pass transmittance through 20 µm of green LPI. Phthalocyanine green plus a yellow pigment pass 500–570 nm and absorb red and blue.
- `ρ_scatter = (0.010, 0.085, 0.030)`: **est.** pigment back-scatter over a black base.
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

There is no silkscreen: the core has no outline or reference-designator graphics yet.

## Sources

- P. B. Johnson and R. W. Christy, "Optical Constants of the Noble Metals", *Phys. Rev. B* 6, 4370 (1972). Au and Cu; tabulated at refractiveindex.info.
- A. D. Rakić et al., "Optical properties of metallic films for vertical-cavity optoelectronic devices", *Applied Optics* 37, 5271 (1998). Ni.
- N. Hoffman, "Physics and Math of Shading", SIGGRAPH Physically Based Shading course notes (2010/2013). The linear-RGB metal F0 method and table.
- E. D. Palik, *Handbook of Optical Constants of Solids*, and the CRC Handbook of Chemistry and Physics. Refractive indices of epoxy, E-glass and silica; Sn, to confirm the tin estimate.
- B. Burley, "Physically-Based Shading at Disney" (2012). Perceptual roughness.
- IPC-4552, ENIG: Ni 3–6 µm, Au 0.04–0.10 µm.
- Manufacturer LPI soldermask datasheets, thickness and gloss/matte texture codes only: Florida CirTech LPI-950 LC-1, LPI-950 C-8GD and LPI-950 S-22.

No EDA tool, EDA library, 3D model or material file was consulted.

## Planned confirmation

The pigment values (mask T and scatter, FR-4 tint, mould compound) and the tin F0 are estimates. The planned confirmation is a photograph of one of our own boards next to an X-Rite/Calibrite colour chart, under known lighting. Fit only those parameters and keep the physical model.
