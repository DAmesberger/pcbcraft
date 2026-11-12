# Datasheet import review: ESP32-S31-WROOM-3

Status: **draft, not published**. Nothing enters a library until a person reviews the rows below and publishes.

Run: model `claude-opus-5-5` via a replay of examples/esp32-s31-wroom-3/wroom3.replay.jsonl (no API calls; usage as recorded), 9 requests; stopped: review report submitted.

Tokens: 0 input, 0 cache write, 0 cache read, 0 output.

## Summary

Component with 99 pins (94 signal pads, the 3 × 3 EPAD as pin 95 and four corner GND pads), IO MUX, LP IO MUX and analog functions per GPIO, 8 pin rules (4 strapping, JTAG, USB Serial/JTAG, UART0, LP UART vs LP I2C), the LGA module package with the antenna keep-out, and the generated 3D model. The footprint and 3D geometry were cross-checked against Espressif's own published footprint and STEP model (kicad-libraries, CC-BY-SA 4.0, used as a dimension source only and not redistributed). Check the open questions first, then each pin row in the Pins tab.

## Sources

- **WROOM3**: ESP32-S31-WROOM-3 & ESP32-S31-WROOM-3U Datasheet v0.7 pre-release, 2026-09-03
- **S31DS**: ESP32-S31 Series Datasheet v0.5 pre-release, 2026-07-13
- **HWDG**: ESP32-S31 Hardware Design Guidelines v0.3, 2026-10-09
- **ESPFP**: Espressif ESP32-S31-WROOM-3 footprint, KiCad format (https://github.com/espressif/kicad-libraries/blob/cfb91e91cc6d6dfa96e0ac3264382b747229e8e7/footprints/Espressif.pretty/ESP32-S31-WROOM-3.kicad_mod) espressif/kicad-libraries commit cfb91e9, 2026-07-24; CC-BY-SA 4.0; dimensions only, not redistributed
- **ESPSTEP**: Espressif ESP32-S31-WROOM-3 3D model ESP32-S31-WROOM-3_V1_1.STEP (https://github.com/espressif/kicad-libraries/blob/cfb91e91cc6d6dfa96e0ac3264382b747229e8e7/3dmodels/espressif.3dshapes/ESP32-S31-WROOM-3_V1_1.STEP) espressif/kicad-libraries commit cfb91e9, 2026-07-24 (exported 2026-06-10); CC-BY-SA 4.0; dimensions only, not redistributed

## Open questions

- [ ] The antenna-side EPAD pad has a 0.5 × 0.5 mm chamfer on its corner towards the antenna (the land pattern's two "1.3" dimensions are its shortened edges, 1.8 - 0.5; Espressif's STEP agrees, its footprint draws 0.45 mm); the module form draws the pad square, 0.125 mm² more copper.
- [ ] The antenna keep-out covers only the 22 × 6 mm antenna area; the hardware guidelines want the antenna at the board edge or a 15 mm clearance cut-out, which the board layout must provide.
- [ ] Peripherals reachable only through the GPIO matrix (UART1+, I2C, LEDC, TWAI, RMT, I2S, ...) are not listed per pin: the TRM chapter is not published yet and the datasheet tables place none of them.
- [ ] Pin directions of IO MUX functions follow the Type columns where readable; LCD, CAM and GMAC directions were inferred from signal names where the type was not extracted.
- [ ] NC pins 33-39: the datasheet does not say what they connect to; they are marked device NC.
- [ ] All three documents are pre-release/preliminary; re-check against the final revision.
- [ ] The -3U variant (24 mm long, U.FL connector, pin 91 ANT2) is not built; only the -3 with PCB antenna.

## Extracted values

| Item | Value | Source | Confidence | Note |
|---|---|---|---|---|
| EPAD chamfer | 0.5 × 0.5 mm on the antenna-side pad's corner towards +y | WROOM3 p.67 Figure 11-1 ("1.3" twice) | Medium | The two "1.3" dimensions run along the chamfered pad's edges: 1.8 - 1.3 = 0.5. Espressif's STEP cuts 0.5 mm; its footprint uses 0.45 (chamfer ratio 0.25). Not drawn: the module form has no chamfered pads. |
| default functions | F0 everywhere except F1 on GPIO20-25; GPIO33/34 USB Serial/JTAG | S31DS p.23 Table 2-3 (bold), Table 2-7 | Medium | Read from bold cells. |
| module outline (-3) | 22 ± 0.2 × 30 ± 0.2 mm | WROOM3 p.64 Figure 10-1 | High |  |
| seated height | 3.5 ± 0.15 mm | WROOM3 p.64 Figure 10-1 side view | High | Espressif's STEP: 3.538 mm overall, inside the tolerance. |
| module board thickness | 1.0 mm | ESPSTEP p.1 module board solid | High | Confirmed: the board solid spans z 0.034-1.038 (1.004 mm over the 0.035 mm pad copper); the datasheet side view's unlabelled "1" is the board. |
| antenna area | 6 mm at the +y end, full width | WROOM3 p.64 Figure 10-1 top view | High |  |
| shield can | 21 × 23 mm, 0.5 mm from the right and bottom edges | WROOM3 p.64 Figure 10-1 top view | High | Centred across (22 - 21 = 1 mm, 0.5 a side); can height 3.5 - 1.0 board = 2.5 mm derived. Espressif's STEP: can 21 × 23 mm centred at (0, -3), z 1.038-3.538 (2.5 mm, 0.2 mm lid). |
| pad count | 94 signal pads, 4 corner pads, EPAD (pin 95): 99 pins | WROOM3 p.14 Table 3-1 | High |  |
| signal pad size | 0.4 × 0.8 mm (0.8 across the edge) | WROOM3 p.64 Figure 10-1 bottom view ("94 x 0.4", "94 x 0.8") | High | Espressif's footprint: every row pad 0.8 × 0.4 mm, turned 90° on the top and bottom rows. |
| pad pitch | 0.85 mm | WROOM3 p.64 Figure 10-1 bottom view | High | Espressif's footprint and STEP: 0.85 mm on every row. |
| side column span | 20.4 mm (25 pads) | WROOM3 p.64 Figure 10-1 bottom view | High | 24 × 0.85 = 20.4 checks. |
| top/bottom row span | 17.85 mm (22 pads) | WROOM3 p.64 Figure 10-1 bottom view | High | 21 × 0.85 = 17.85 checks. |
| corner pads | 4 × 0.8 × 0.8 mm at 20.6 × 22.6 mm centres | WROOM3 p.64 Figure 10-1 bottom view | High |  |
| corner pad inset | 0.7 mm from the body edges | ESPFP p.1 pads 96-99 | High | Espressif's footprint: pads 96-99 0.8 × 0.8 at (±10.3, -14.3) and (±10.3, 8.3) in the module frame. |
| pad body centre | y = -3 mm (the 24 mm pad body below the 6 mm antenna area) | ESPFP p.1 footprint origin and outline | High | Espressif's footprint origin is the pad-body centre, with the outline at y -18…12 (y down): 3 mm below the module centre. The STEP agrees. |
| pin 1 position | left column, first pad below corner pad 99: (-10.3, 7.2) | ESPFP p.1 pad 1 | High | Espressif's footprint: pad 1 at (-10.3, -10.2), y down from the pad-body centre: (-10.3, 7.2) here; 1.1 mm below corner pad 99, pad 94 1.375 mm from it. |
| numbering | 1-25 left top to bottom, 26-47 bottom left to right, 48-72 right bottom to top, 73-94 top right to left; corners 96 BL, 97 BR, 98 TR, 99 TL | WROOM3 p.12 Figure 3-1 (top view) | High |  |
| EPAD | 9 × 1.8 mm squares, 0.55 mm gaps (2.35 mm pitch), 3 × 3 turned 45°, 6.5 mm across, centred on the pad body | ESPFP p.1 pad 95 (nine pads) | High | Espressif's footprint: nine 1.8 mm pads turned 45° at (0, 0), (±1.6617, ±1.6617), (±3.3234, 0) and (0, ±3.3234) about the pad-body centre (2.35 mm pitch along the turned axes, 0.55 mm gaps); the STEP agrees. The pad nearest the antenna is chamfered (see the open question); drawn square here. |
| can mark | Ø0.5 mm hole in the can lid at (-9.5, -13.5), the -x/-y corner by pins 25/26 | ESPSTEP p.1 shield can lid | High | Matches the Ø0.5 in the datasheet's top view (Figure 10-1). It is not a pin-1 mark (pin 1 is at the +y end of the left row), so the can carries no polarity mark here; Espressif's footprint has no pin-1 silkscreen mark either (Sparksmith's footprint adds its own at pad 1). |
| land pattern | equal to the module pads (no extension given) | WROOM3 p.67 Figure 11-1 | High |  |
| antenna keep-out | the 22 × 6 mm antenna area on every copper layer | ESPFP p.1 zone "antenna keepout" | High | Espressif's footprint: a keep-out over the 22 × 6 mm antenna area on the outer and first inner copper layers (no tracks, vias, pads, pours or parts). The guidelines also ask for the antenna at the board edge or a cut-out with at least 15 mm clearance; not modelled. |
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

## Pin rules

Each rule evaluated without selections, with each function, peripheral or role it names made active, and with all of them together.

| Rule | Without selections | Samples | Verdicts | Failures |
|---|---|---|---|---|
| strap_io61 | ok | 2 | ok, warn |  |
| strap_io60 | ok | 2 | ok, warn |  |
| strap_io36 | ok | 6 | ok, warn |  |
| strap_io37 | ok | 7 | ok, warn |  |
| jtag_pins | ok | 6 | ok |  |
| usb_serial_jtag_pins | ok | 4 | ok |  |
| uart0_pins | ok | 4 | ok |  |
| lp_uart_flow_excludes_lp_i2c | ok | 7 | forbid, ok |  |

## Drafts

- component 1 ‘ESP32-S31-WROOM-3’: pinout.unreviewed
- symbol 2 ‘ESP32-S31-WROOM-3 symbol’: 
- package 3 ‘ESP32-S31-WROOM-3 WROOM-3’: 
- binding 4 ‘ESP32-S31-WROOM-3 WROOM-3 binding’: 
- footprint 5 ‘ESP32-S31-WROOM-3 WROOM-3 footprint’: 

## Steps

1. library_pinout_apply: revision 1 (outcome, report)
1. library_package_from_template: revision 2 (outcome, plan; Terminals matched by number: 99 kept, 0 added, 0 removed.)
1. library_set_marking: revision 3 (outcome)

## Pins

One row per package pin as imported; check each against the datasheet in the Pins tab. `sparksmith review-diff A.pcbcraft B.pcbcraft` lists two runs' differences pin by pin.

| Pin | Name | Default | Functions [mux] peripheral.role | Source | Flags |
|---|---|---|---|---|---|
| 1 | GND | gnd | gnd power_input | p.14 Table 3-1 | unreviewed |
| 2 | GND | gnd | gnd power_input | p.14 Table 3-1 | unreviewed |
| 3 | 3V3 | vdd | vdd power_input | p.14 Table 3-1 | unreviewed |
| 4 | 3V3 | vdd | vdd power_input | p.14 Table 3-1 | unreviewed |
| 5 | EN | en | en input | p.14 Table 3-1 | unreviewed |
| 6 | IO2 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; lcd_data19_out [F3] LCD.data19_out output; lp_uart_dtrn [LP_F0] LP_UART.dtrn output; lp_spi_ck [LP_F2] LP_SPI.ck output; lp_probe_out2 [LP_F3] LP_PROBE.out2 output; lp_gpio [LP_F1] bidirectional | p.14 Table 3-1 | unreviewed |
| 7 | IO3 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; lcd_data20_out [F3] LCD.data20_out output; lp_uart_dsrn [LP_F0] LP_UART.dsrn input; lp_spi_cs [LP_F2] LP_SPI.cs output; lp_probe_out3 [LP_F3] LP_PROBE.out3 output; lp_gpio [LP_F1] bidirectional | p.14 Table 3-1 | unreviewed |
| 8 | IO0 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; lp_probe_out0 [LP_F3] LP_PROBE.out0 output; xtal_32k_n [ANALOG] XTAL_32K.n passive; lp_gpio [LP_F1] bidirectional | p.14 Table 3-1 | unreviewed |
| 9 | IO1 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; lp_probe_out1 [LP_F3] LP_PROBE.out1 output; xtal_32k_p [ANALOG] XTAL_32K.p passive; lp_gpio [LP_F1] bidirectional | p.14 Table 3-1 | unreviewed |
| 10 | IO4 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; lcd_data21_out [F3] LCD.data21_out output; lp_uart_rtsn [LP_F0] LP_UART.rtsn output; lp_spi_d [LP_F2] LP_SPI.d output; lp_probe_out4 [LP_F3] LP_PROBE.out4 output; lp_gpio [LP_F1] bidirectional | p.14 Table 3-1 | unreviewed |
| 11 | IO5 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; lcd_data22_out [F3] LCD.data22_out output; lp_uart_ctsn [LP_F0] LP_UART.ctsn input; lp_spi_q [LP_F2] LP_SPI.q input; lp_probe_out5 [LP_F3] LP_PROBE.out5 output; lp_gpio [LP_F1] bidirectional | p.14 Table 3-1 | unreviewed |
| 12 | IO6 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; lp_uart_txd [LP_F0] LP_UART.txd output; lp_i2c_scl [LP_F2] LP_I2C.scl bidirectional; lp_probe_out6 [LP_F3] LP_PROBE.out6 output; touch_ch0 [ANALOG] TOUCH.ch0 input; lp_gpio [LP_F1] bidirectional | p.14 Table 3-1 | unreviewed |
| 13 | IO7 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; lcd_data23_out [F3] LCD.data23_out output; lp_uart_rxd [LP_F0] LP_UART.rxd input; lp_i2c_sda [LP_F2] LP_I2C.sda bidirectional; lp_probe_out7 [LP_F3] LP_PROBE.out7 output; touch_ch1 [ANALOG] TOUCH.ch1 input; lp_gpio [LP_F1] bidirectional | p.14 Table 3-1 | unreviewed |
| 14 | IO8 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; gmac_phy_txd0 [F2] GMAC.phy_txd0 output; lcd_data0_out [F3] LCD.data0_out output; touch_ch2 [ANALOG] TOUCH.ch2 input | p.14 Table 3-1 | unreviewed |
| 15 | IO9 | spi2_hold | gpio_in [F1] input; gpio_out [F1] output; spi2_hold [F0] SPI2.hold bidirectional; gmac_phy_txd1 [F2] GMAC.phy_txd1 output; lcd_data1_out [F3] LCD.data1_out output; touch_ch3 [ANALOG] TOUCH.ch3 input | p.14 Table 3-1 | unreviewed |
| 16 | IO10 | spi2_cs | gpio_in [F1] input; gpio_out [F1] output; spi2_cs [F0] SPI2.cs bidirectional; gmac_phy_txd2 [F2] GMAC.phy_txd2 output; lcd_data2_out [F3] LCD.data2_out output; touch_ch4 [ANALOG] TOUCH.ch4 input | p.14 Table 3-1 | unreviewed |
| 17 | IO11 | spi2_d | gpio_in [F1] input; gpio_out [F1] output; spi2_d [F0] SPI2.d bidirectional; gmac_phy_txd3 [F2] GMAC.phy_txd3 output; lcd_data3_out [F3] LCD.data3_out output; touch_ch5 [ANALOG] TOUCH.ch5 input | p.14 Table 3-1 | unreviewed |
| 18 | IO12 | spi2_ck | gpio_in [F1] input; gpio_out [F1] output; spi2_ck [F0] SPI2.ck bidirectional; gmac_phy_txen [F2] GMAC.phy_txen output; lcd_data4_out [F3] LCD.data4_out output; touch_ch6 [ANALOG] TOUCH.ch6 input | p.14 Table 3-1 | unreviewed |
| 19 | IO13 | spi2_q | gpio_in [F1] input; gpio_out [F1] output; spi2_q [F0] SPI2.q bidirectional; gmac_rmii_clk [F2] GMAC.rmii_clk bidirectional; lcd_data5_out [F3] LCD.data5_out output; touch_ch7 [ANALOG] TOUCH.ch7 input | p.14 Table 3-1 | unreviewed |
| 20 | IO14 | spi2_wp | gpio_in [F1] input; gpio_out [F1] output; spi2_wp [F0] SPI2.wp bidirectional; gmac_rx_clk [F2] GMAC.rx_clk input; lcd_data6_out [F3] LCD.data6_out output; touch_ch8 [ANALOG] TOUCH.ch8 input | p.14 Table 3-1 | unreviewed |
| 21 | IO15 | spi2_io4 | gpio_in [F1] input; gpio_out [F1] output; spi2_io4 [F0] SPI2.io4 bidirectional; gmac_phy_rxdv [F2] GMAC.phy_rxdv input; lcd_data7_out [F3] LCD.data7_out output; touch_ch9 [ANALOG] TOUCH.ch9 input | p.14 Table 3-1 | unreviewed |
| 22 | IO16 | spi2_io5 | gpio_in [F1] input; gpio_out [F1] output; spi2_io5 [F0] SPI2.io5 bidirectional; gmac_phy_rxd3 [F2] GMAC.phy_rxd3 input; lcd_data8_out [F3] LCD.data8_out output; touch_ch10 [ANALOG] TOUCH.ch10 input | p.14 Table 3-1 | unreviewed |
| 23 | IO17 | spi2_io6 | gpio_in [F1] input; gpio_out [F1] output; spi2_io6 [F0] SPI2.io6 bidirectional; gmac_phy_rxd2 [F2] GMAC.phy_rxd2 input; lcd_data9_out [F3] LCD.data9_out output; touch_ch11 [ANALOG] TOUCH.ch11 input | p.14 Table 3-1 | unreviewed |
| 24 | IO18 | spi2_io7 | gpio_in [F1] input; gpio_out [F1] output; spi2_io7 [F0] SPI2.io7 bidirectional; gmac_phy_rxd1 [F2] GMAC.phy_rxd1 input; lcd_data10_out [F3] LCD.data10_out output; touch_ch12 [ANALOG] TOUCH.ch12 input | p.14 Table 3-1 | unreviewed |
| 25 | IO19 | spi2_dqs | gpio_in [F1] input; gpio_out [F1] output; spi2_dqs [F0] SPI2.dqs output; gmac_phy_rxd0 [F2] GMAC.phy_rxd0 input; lcd_data11_out [F3] LCD.data11_out output; touch_ch13 [ANALOG] TOUCH.ch13 input | p.14 Table 3-1 | unreviewed |
| 26 | GND | gnd | gnd power_input | p.14 Table 3-1 | unreviewed |
| 27 | SD_D0 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; sdio_data0 [F0] SDIO.data0 bidirectional; spi2_ck [F2] SPI2.ck bidirectional | p.14 Table 3-1 | unreviewed |
| 28 | SD_D1 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; sdio_data1 [F0] SDIO.data1 bidirectional; spi2_d [F2] SPI2.d bidirectional | p.14 Table 3-1 | unreviewed |
| 29 | SD_D2 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; sdio_data2 [F0] SDIO.data2 bidirectional; spi2_q [F2] SPI2.q bidirectional | p.14 Table 3-1 | unreviewed |
| 30 | SD_D3 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; sdio_data3 [F0] SDIO.data3 bidirectional; spi2_cs [F2] SPI2.cs bidirectional | p.14 Table 3-1 | unreviewed |
| 31 | SD_CLK | gpio_in | gpio_in [F1] input; gpio_out [F1] output; sdio_clk [F0] SDIO.clk output; spi2_hold [F2] SPI2.hold bidirectional | p.14 Table 3-1 | unreviewed |
| 32 | SD_CMD | gpio_in | gpio_in [F1] input; gpio_out [F1] output; sdio_cmd [F0] SDIO.cmd bidirectional; spi2_wp [F2] SPI2.wp bidirectional | p.14 Table 3-1 | unreviewed |
| 33 | NC |  |  |  | device_nc, unreviewed |
| 34 | NC |  |  |  | device_nc, unreviewed |
| 35 | NC |  |  |  | device_nc, unreviewed |
| 36 | NC |  |  |  | device_nc, unreviewed |
| 37 | NC |  |  |  | device_nc, unreviewed |
| 38 | NC |  |  |  | device_nc, unreviewed |
| 39 | NC |  |  |  | device_nc, unreviewed |
| 40 | USB_DP | usb_otg_hs_dp | usb_otg_hs_dp USB_OTG_HS.dp bidirectional | p.14 Table 3-1 | unreviewed |
| 41 | USB_DM | usb_otg_hs_dm | usb_otg_hs_dm USB_OTG_HS.dm bidirectional | p.14 Table 3-1 | unreviewed |
| 42 | IO33 | usb_serial_jtag_dm | gpio_in [F1] input; gpio_out [F1] output; lcd_data12_out [F3] LCD.data12_out output; usb_serial_jtag_dm [ANALOG] USB_SERIAL_JTAG.dm bidirectional | p.14 Table 3-1 | unreviewed |
| 43 | IO34 | usb_serial_jtag_dp | gpio_in [F1] input; gpio_out [F1] output; lcd_data13_out [F3] LCD.data13_out output; usb_serial_jtag_dp [ANALOG] USB_SERIAL_JTAG.dp bidirectional | p.14 Table 3-1 | unreviewed |
| 44 | IO35 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; gmac_ref_clk [F2] GMAC.ref_clk bidirectional; lcd_data14_out [F3] LCD.data14_out output; sd2_cdata0 [F4] SD2.cdata0 bidirectional | p.14 Table 3-1 | unreviewed |
| 45 | IO36 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; lcd_data15_out [F3] LCD.data15_out output; sd2_cdata1 [F4] SD2.cdata1 bidirectional | p.14 Table 3-1 | unreviewed |
| 46 | IO37 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; lcd_data16_out [F3] LCD.data16_out output; sd2_cdata2 [F4] SD2.cdata2 bidirectional; pad_comp_comp0 [ANALOG] PAD_COMP.comp0 input | p.14 Table 3-1 | unreviewed |
| 47 | GND | gnd | gnd power_input | p.14 Table 3-1 | unreviewed |
| 48 | GND | gnd | gnd power_input | p.16 Table 3-1 | unreviewed |
| 49 | IO38 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; lcd_data17_out [F3] LCD.data17_out output; sd2_cdata3 [F4] SD2.cdata3 bidirectional; pad_comp_comp1 [ANALOG] PAD_COMP.comp1 input | p.16 Table 3-1 | unreviewed |
| 50 | IO39 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; lcd_data18_out [F3] LCD.data18_out output; sd2_cclk [F4] SD2.cclk output; pad_comp_comp2 [ANALOG] PAD_COMP.comp2 input | p.16 Table 3-1 | unreviewed |
| 51 | IO40 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; lcd_pclk [F3] LCD.pclk output; sd2_ccmd [F4] SD2.ccmd bidirectional; pad_comp_comp3 [ANALOG] PAD_COMP.comp3 input | p.16 Table 3-1 | unreviewed |
| 52 | IO42 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; adc1_ch0_n [ANALOG] ADC1.ch0_n input | p.16 Table 3-1 | unreviewed |
| 53 | IO43 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; lcd_h_enable [F3] LCD.h_enable output; adc1_ch0_p [ANALOG] ADC1.ch0_p input | p.16 Table 3-1 | unreviewed |
| 54 | IO44 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; lcd_h_sync [F3] LCD.h_sync output; adc1_ch1_n [ANALOG] ADC1.ch1_n input | p.16 Table 3-1 | unreviewed |
| 55 | IO45 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; lcd_v_sync [F3] LCD.v_sync output; adc1_ch1_p [ANALOG] ADC1.ch1_p input | p.16 Table 3-1 | unreviewed |
| 56 | IO46 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; cam_data0_in [F3] CAM.data0_in input; adc1_ch2_n [ANALOG] ADC1.ch2_n input | p.16 Table 3-1 | unreviewed |
| 57 | IO47 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; cam_data1_in [F3] CAM.data1_in input; adc1_ch2_p [ANALOG] ADC1.ch2_p input | p.16 Table 3-1 | unreviewed |
| 58 | IO48 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; cam_data2_in [F3] CAM.data2_in input; adc1_ch3_n [ANALOG] ADC1.ch3_n input | p.16 Table 3-1 | unreviewed |
| 59 | IO49 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; cam_data3_in [F3] CAM.data3_in input; adc1_ch3_p [ANALOG] ADC1.ch3_p input | p.16 Table 3-1 | unreviewed |
| 60 | IO50 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; cam_data4_in [F3] CAM.data4_in input; adc2_ch0_n [ANALOG] ADC2.ch0_n input | p.16 Table 3-1 | unreviewed |
| 61 | IO51 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; cam_data5_in [F3] CAM.data5_in input; adc2_ch0_p [ANALOG] ADC2.ch0_p input | p.16 Table 3-1 | unreviewed |
| 62 | IO52 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; spi2_cs [F2] SPI2.cs bidirectional; cam_data6_in [F3] CAM.data6_in input; adc2_ch1_n [ANALOG] ADC2.ch1_n input | p.16 Table 3-1 | unreviewed |
| 63 | IO53 | gpio_in | gpio_in [F1] input; gpio_out [F1] output; spi2_ck [F2] SPI2.ck bidirectional; cam_data7_in [F3] CAM.data7_in input; adc2_ch1_p [ANALOG] ADC2.ch1_p input | p.16 Table 3-1 | unreviewed |
| 64 | IO54 | jtag_mtdo | gpio_in [F1] input; gpio_out [F1] output; jtag_mtdo [F0] JTAG.mtdo output; spi2_d [F2] SPI2.d bidirectional; cam_pclk [F3] CAM.pclk input; adc2_ch2_n [ANALOG] ADC2.ch2_n input | p.16 Table 3-1 | unreviewed |
| 65 | IO55 | jtag_mtck | gpio_in [F1] input; gpio_out [F1] output; jtag_mtck [F0] JTAG.mtck input; spi2_q [F2] SPI2.q bidirectional; cam_xclk [F3] CAM.xclk output; adc2_ch2_p [ANALOG] ADC2.ch2_p input | p.16 Table 3-1 | unreviewed |
| 66 | IO56 | jtag_mtdi | gpio_in [F1] input; gpio_out [F1] output; jtag_mtdi [F0] JTAG.mtdi input; spi2_hold [F2] SPI2.hold bidirectional; cam_v_sync [F3] CAM.v_sync input; adc2_ch3_n [ANALOG] ADC2.ch3_n input | p.16 Table 3-1 | unreviewed |
| 67 | IO57 | jtag_mtms | gpio_in [F1] input; gpio_out [F1] output; jtag_mtms [F0] JTAG.mtms input; spi2_wp [F2] SPI2.wp bidirectional; cam_h_sync [F3] CAM.h_sync input; adc2_ch3_p [ANALOG] ADC2.ch3_p input | p.16 Table 3-1 | unreviewed |
| 68 | TX0 | uart0_txd | gpio_in [F1] input; gpio_out [F1] output; uart0_txd [F0] UART0.txd output | p.16 Table 3-1 | unreviewed |
| 69 | RX0 | uart0_rxd | gpio_in [F1] input; gpio_out [F1] output; uart0_rxd [F0] UART0.rxd input | p.16 Table 3-1 | unreviewed |
| 70 | IO60 | gpio_in | gpio_in [F1] input; gpio_out [F1] output | p.16 Table 3-1 | unreviewed |
| 71 | IO61 | gpio_in | gpio_in [F1] input; gpio_out [F1] output | p.16 Table 3-1 | unreviewed |
| 72 | GND | gnd | gnd power_input | p.16 Table 3-1 | unreviewed |
| 73 | GND | gnd | gnd power_input | p.16 Table 3-1 | unreviewed |
| 74 | GND | gnd | gnd power_input | p.16 Table 3-1 | unreviewed |
| 75 | GND | gnd | gnd power_input | p.16 Table 3-1 | unreviewed |
| 76 | GND | gnd | gnd power_input | p.16 Table 3-1 | unreviewed |
| 77 | GND | gnd | gnd power_input | p.16 Table 3-1 | unreviewed |
| 78 | GND | gnd | gnd power_input | p.16 Table 3-1 | unreviewed |
| 79 | GND | gnd | gnd power_input | p.16 Table 3-1 | unreviewed |
| 80 | GND | gnd | gnd power_input | p.16 Table 3-1 | unreviewed |
| 81 | GND | gnd | gnd power_input | p.16 Table 3-1 | unreviewed |
| 82 | GND | gnd | gnd power_input | p.16 Table 3-1 | unreviewed |
| 83 | GND | gnd | gnd power_input | p.16 Table 3-1 | unreviewed |
| 84 | GND | gnd | gnd power_input | p.16 Table 3-1 | unreviewed |
| 85 | GND | gnd | gnd power_input | p.16 Table 3-1 | unreviewed |
| 86 | GND | gnd | gnd power_input | p.16 Table 3-1 | unreviewed |
| 87 | GND | gnd | gnd power_input | p.16 Table 3-1 | unreviewed |
| 88 | GND | gnd | gnd power_input | p.16 Table 3-1 | unreviewed |
| 89 | GND | gnd | gnd power_input | p.16 Table 3-1 | unreviewed |
| 90 | GND | gnd | gnd power_input | p.16 Table 3-1 | unreviewed |
| 91 | NC |  |  |  | device_nc, unreviewed |
| 92 | GND | gnd | gnd power_input | p.16 Table 3-1 | unreviewed |
| 93 | GND | gnd | gnd power_input | p.16 Table 3-1 | unreviewed |
| 94 | GND | gnd | gnd power_input | p.16 Table 3-1 | unreviewed |
| 95 | GND | gnd | gnd power_input | p.64 Figure 10-1 | unreviewed |
| 96 | GND | gnd | gnd power_input | p.64 Figure 10-1 | unreviewed |
| 97 | GND | gnd | gnd power_input | p.64 Figure 10-1 | unreviewed |
| 98 | GND | gnd | gnd power_input | p.64 Figure 10-1 | unreviewed |
| 99 | GND | gnd | gnd power_input | p.64 Figure 10-1 | unreviewed |

## Requests

| # | Stop | Cache write | Cache read | Input | Output | Tools |
|---|---|---|---|---|---|---|
| 1 | tool_use | 0 | 0 | 0 | 0 | library_pinout_schema |
| 2 | tool_use | 0 | 0 | 0 | 0 | library_pinout_preview |
| 3 | tool_use | 0 | 0 | 0 | 0 | library_pinout_apply |
| 4 | tool_use | 0 | 0 | 0 | 0 | library_package_schema |
| 5 | tool_use | 0 | 0 | 0 | 0 | library_package_plan |
| 6 | tool_use | 0 | 0 | 0 | 0 | library_package_from_template |
| 7 | tool_use | 0 | 0 | 0 | 0 | library_set_marking |
| 8 | tool_use | 0 | 0 | 0 | 0 | component_check_drafts |
| 9 | tool_use | 0 | 0 | 0 | 0 | submit_review |
