# Datasheet import review: ESP32-S31-WROOM-3

Status: **draft, not published**. Nothing enters a library until a person reviews the rows below and publishes.

Run: model `claude-opus-5-5` via a replay of wroom3/wroom3.replay.jsonl (no API calls; spend not real), 9 requests, spend $0.00 of the $15.00 cap; stopped: review report submitted.

Tokens: 0 input, 0 cache write, 0 cache read, 0 output.

## Summary

Component with 99 pins (94 signal pads, the 3 × 3 EPAD as pin 95 and four corner GND pads), IO MUX, LP IO MUX and analog functions per GPIO, 8 pin rules (4 strapping, JTAG, USB Serial/JTAG, UART0, LP UART vs LP I2C), the LGA module package with the antenna keep-out, and the generated 3D model. Check the open questions first, then each pin row in the Pins tab.

## Sources

- **WROOM3**: ESP32-S31-WROOM-3 & ESP32-S31-WROOM-3U Datasheet v0.7 pre-release, 2026-09-03
- **S31DS**: ESP32-S31 Series Datasheet v0.5 pre-release, 2026-07-13
- **HWDG**: ESP32-S31 Hardware Design Guidelines v0.3, 2026-10-09

## Open questions

- [ ] Board thickness 1.0 mm is read from an unlabelled side-view dimension; confirm with Espressif or a measured sample.
- [ ] The EPAD centre is taken on the 22 × 24 pad body centre lines (y = -3); confirm against the land pattern (Figure 11-1).
- [ ] One EPAD square towards the antenna is chamfered ("1.3" dimensions); the footprint uses a full square.
- [ ] The antenna keep-out covers only the 22 × 6 mm antenna area; the hardware guidelines want the antenna at the board edge or a 15 mm clearance cut-out, which the board layout must provide.
- [ ] Peripherals reachable only through the GPIO matrix (UART1+, I2C, LEDC, TWAI, RMT, I2S, ...) are not listed per pin: the TRM chapter is not published yet and the datasheet tables place none of them.
- [ ] Pin directions of IO MUX functions follow the Type columns where readable; LCD, CAM and GMAC directions were inferred from signal names where the type was not extracted.
- [ ] NC pins 33-39: the datasheet does not say what they connect to; they are marked device NC.
- [ ] All three documents are pre-release/preliminary; re-check against the final revision.
- [ ] The -3U variant (24 mm long, U.FL connector, pin 91 ANT2) is not built; only the -3 with PCB antenna.

## Extracted values

| Item | Value | Source | Confidence | Note |
|---|---|---|---|---|
| module board thickness | 1.0 mm | WROOM3 p.64 Figure 10-1 side view | Medium | The side view dimension "1" is not labelled; read as the board. |
| corner pad inset | 0.7 mm from the body edges | WROOM3 p.64 Figure 10-1 bottom view | Medium | Consistent with (22 - 20.6)/2 = 0.7; the 22.6 span sits in the 24 mm pad body, top edge at the antenna boundary. |
| pad body centre | y = -3 mm (the 24 mm pad body below the 6 mm antenna area) | WROOM3 p.64 Figure 10-1 | Medium | Derived: 30/2 - 6 - 24/2 = -3. |
| pin 1 position | left column, first pad below corner pad 99: (-10.3, 7.2) | WROOM3 p.12 Figure 3-1 | Medium | Derived from the 20.4 span centred on the pad body. |
| EPAD | 9 × 1.8 mm squares, 0.55 mm gaps (2.35 mm pitch), 3 × 3 turned 45°, 6.5 mm across, centred on the pad body | WROOM3 p.64 Figure 10-1 bottom view | Medium | Centre on the dashed centre lines of the 22 × 24 body; one antenna-side pad is chamfered ("1.3"), drawn square here. |
| antenna keep-out | the 22 × 6 mm antenna area on every copper layer | WROOM3 p.12 Figure 3-1 "Keepout Zone" | Medium | The guidelines also ask for the antenna at the board edge or a cut-out with at least 15 mm clearance; not modelled. |
| default functions | F0 everywhere except F1 on GPIO20-25; GPIO33/34 USB Serial/JTAG | S31DS p.23 Table 2-3 (bold), Table 2-7 | Medium | Read from bold cells. |
| module outline (-3) | 22 ± 0.2 × 30 ± 0.2 mm | WROOM3 p.64 Figure 10-1 | High |  |
| seated height | 3.5 ± 0.15 mm | WROOM3 p.64 Figure 10-1 side view | High |  |
| antenna area | 6 mm at the +y end, full width | WROOM3 p.64 Figure 10-1 top view | High |  |
| shield can | 21 × 23 mm, 0.5 mm from the right and bottom edges | WROOM3 p.64 Figure 10-1 top view | High | Centred across (22 - 21 = 1 mm, 0.5 a side); can height 3.5 - 1.0 board = 2.5 mm derived. |
| pad count | 94 signal pads, 4 corner pads, EPAD (pin 95): 99 pins | WROOM3 p.14 Table 3-1 | High |  |
| signal pad size | 0.4 × 0.8 mm (0.8 across the edge) | WROOM3 p.64 Figure 10-1 bottom view ("94 x 0.4", "94 x 0.8") | High |  |
| pad pitch | 0.85 mm | WROOM3 p.64 Figure 10-1 bottom view | High |  |
| side column span | 20.4 mm (25 pads) | WROOM3 p.64 Figure 10-1 bottom view | High | 24 × 0.85 = 20.4 checks. |
| top/bottom row span | 17.85 mm (22 pads) | WROOM3 p.64 Figure 10-1 bottom view | High | 21 × 0.85 = 17.85 checks. |
| corner pads | 4 × 0.8 × 0.8 mm at 20.6 × 22.6 mm centres | WROOM3 p.64 Figure 10-1 bottom view | High |  |
| numbering | 1-25 left top to bottom, 26-47 bottom left to right, 48-72 right bottom to top, 73-94 top right to left; corners 96 BL, 97 BR, 98 TR, 99 TL | WROOM3 p.12 Figure 3-1 (top view) | High |  |
| land pattern | equal to the module pads (no extension given) | WROOM3 p.67 Figure 11-1 | High |  |
| antenna placement | antenna outside the base board or board cut-out, min 15 mm clearance | HWDG p.28 Figure 23 | High |  |
| IO MUX functions | F0-F4 per GPIO | S31DS p.23 Table 2-3 | High |  |
| LP IO MUX functions | GPIO0-7 LP_F0-LP_F3 | S31DS p.26 Table 2-5 | High |  |
| analog functions | XTAL_32K, TOUCH_CH0-13, USB1P1, PAD_COMP0-3, ADC1/ADC2 channels | S31DS p.27 Table 2-7 | High |  |
| strapping IO61 | weak pull-up, default 1; 1 SPI boot, 0 (IO60 = 1) joint download, both 0 invalid | S31DS p.36 Tables 3-1, 3-3 | High |  |
| strapping IO60 | weak pull-up, default 1; boot mode and UART0 ROM print | S31DS p.36 Tables 3-1, 3-3, 3-5 | High |  |
| strapping IO36 | floating on the chip, pulled up in the module; VDD_SPI 1 = 3.3 V, 0 = 1.8 V (EFUSE_PMU_FLASH_POWER_SEL = 0) | S31DS p.38 Table 3-4 | High |  |
| strapping IO37 | no pull; JTAG source when EFUSE_JTAG_SEL_ENABLE = 1 | S31DS p.40 Table 3-7 | High |  |
| module strapping pins | IO36 (45), IO37 (46), IO60 (70), IO61 (71) | WROOM3 p.18 Table 4-1 | High |  |
| restricted pins | flash GPIO26-32 not on the module; JTAG GPIO54-57; USB Serial/JTAG GPIO33/34; UART0 GPIO58/59 | S31DS p.29 Section 2.3.4 | High |  |
| LP UART flow control vs LP I2C | cannot be used together | S31DS p.26 Table 2-5 note | High |  |
| EN | do not leave floating; RC 10 kOhm / 1 uF | WROOM3 p.63 Section 9 | High |  |

## Drafts

- component 1 ‘ESP32-S31-WROOM-3’: pinout.unreviewed
- symbol 2 ‘ESP32-S31-WROOM-3 symbol’: 
- package 3 ‘ESP32-S31-WROOM-3 WROOM-3’: 
- binding 4 ‘ESP32-S31-WROOM-3 WROOM-3 binding’: 
- footprint 5 ‘ESP32-S31-WROOM-3 WROOM-3 footprint’: 

## Steps

1. library_pinout_apply: revision 1 (outcome, report)
1. library_package_from_template: revision 2 (outcome, plan)
1. library_set_marking: revision 3 (outcome)
