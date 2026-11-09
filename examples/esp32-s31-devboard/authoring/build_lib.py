"""Build the dev board's parts in the session (stage 1)."""
from decimal import Decimal

from stage_lib import *

PROV_NOTE = "Authored for the ESP32-S31 dev board e2e run from the manufacturer's public datasheet; checked by the author, not by a second reviewer."


def quantity(s, name, dim, symbol, aliases):
    body = {"units": {"dimension": dim, "units": [{"symbol": symbol, "aliases": aliases, "scale": 1.0, "si_prefixes": True}]},
            "default_unit": symbol}
    r = s.op("library.new_draft", {"meta": meta(name, "SI quantity"), "definition": {"kind": "quantity_kind", "body": body}})
    return r["result"]["outcome"]["id"]


def field(s, name, kind):
    body = {"next_local": 1, "value_type": {"type": "quantity", "kind": {"id": kind, "revision": 1}, "min": 0.0, "max": None},
            "default": None, "required_at": None}
    r = s.op("library.new_draft", {"meta": meta(name, "Project field"), "definition": {"kind": "field", "body": body}})
    return r["result"]["outcome"]["id"]


def set_fields(s, comp, fields, name=None, prov=None):
    d = drafts(s)[comp]
    d["definition"]["body"]["fields"] = [{"field": {"id": f, "revision": 1}, "default": None, "show_on_symbol": show}
                                         for f, show in fields]
    m = d["meta"]
    if name: m["name"] = name
    if prov: m["provenance"] = prov
    edit(s, [{"id": comp, "meta": m, "definition": d["definition"]}], f"fields of {comp}")


def part(s, mpn, manufacturer, comp, binding, specs, ds_title, ds_url, ds_rev=None, desc=""):
    body = {"next_local": 2, "manufacturer": manufacturer, "mpn": mpn,
            "component": {"id": comp, "revision": 1}, "binding": {"id": binding, "revision": 1},
            "specs": [{"field": {"id": f, "revision": 1}, "value": {"type": "quantity", "value": {"si": format(Decimal(str(v)).normalize(), "f"), "display_unit": None}},
                       "meaning": m} for f, v, m in specs],
            "references": [{"id": 1, "kind": "datasheet", "title": ds_title, "url": ds_url, "document_revision": ds_rev,
                            "asset": None, "licence": None,
                            "provenance": {"origin": None, "basis": {"basis": "authored"}, "licence": None}}],
            "offers": [], "provenance": {"origin": None, "basis": {"basis": "authored"}, "licence": None}}
    r = s.op("library.new_draft", {"meta": meta(mpn, PROV_NOTE, desc), "definition": {"kind": "part", "body": body}}, f"part {mpn}")
    return r["result"]["outcome"]["id"]




def replace_pads(s, fp, pads, prov=None):
    """Replace a template footprint's pads (keeping ids and terminals in order)."""
    d = drafts(s)[fp]
    body = d["definition"]["body"]
    for p, (at, size) in zip(body["pads"], pads):
        p["position"] = at
        p["shape"] = {"rect": {"size": size}}
    if prov:
        d["meta"]["provenance"] = prov
    edit(s, [{"id": fp, "meta": d["meta"], "definition": d["definition"]}], f"pads of {fp}")


def set_graphics(s, fp, keep_text=True, extra=(), silk_text_at=None):
    d = drafts(s)[fp]
    body = d["definition"]["body"]
    n = body["next_local"]
    gfx = [g for g in body["graphics"] if keep_text and "text" in g["item"]]
    if silk_text_at is not None:
        for g in gfx:
            if g["layer"] == "silkscreen":
                g["item"]["text"]["at"] = silk_text_at
    for g in extra:
        gfx.append(dict(g, id=n)); n += 1
    body["graphics"] = gfx
    body["next_local"] = n
    edit(s, [{"id": fp, "meta": d["meta"], "definition": d["definition"]}], f"graphics of {fp}")


def publish_and_close(s, ids, note):
    """Publish; library.publish closes the drafts it published by default,
    so they do not count against the project's 64-draft limit."""
    s.op("library.publish", {"drafts": ids}, note)


USB_C_ROW = "USB-C receptacle, USB 2.0, 16 contacts, top mount (GCT USB4105)"


def drafts_name(s, i):
    return drafts(s)[i]["meta"]["name"]


def build(s):
    L = {}
    # The module: the datasheet assistant's reviewed drafts, published here.
    publish_and_close(s, [1, 2, 3, 4, 5, 6], "publish ESP32-S31-WROOM-3")
    L["module"] = {"component": 1, "symbol": 2, "binding": 4, "footprint": 5}

    # Typed fields.
    res_k = quantity(s, "Resistance", [2, 1, -3, -2, 0, 0, 0], "Ω", ["ohm", "R"])
    cap_k = quantity(s, "Capacitance", [-2, -1, 4, 2, 0, 0, 0], "F", [])
    vol_k = quantity(s, "Voltage", [2, 1, -3, -1, 0, 0, 0], "V", [])
    publish_and_close(s, [res_k, cap_k, vol_k], "publish quantity kinds")
    F = {"res": field(s, "Resistance", res_k), "cap": field(s, "Capacitance", cap_k), "volt": field(s, "Rated voltage", vol_k)}
    publish_and_close(s, list(F.values()), "publish fields")
    L["fields"] = F

    publish = []

    # Generic 0603 resistor, generic 0603 and 0805 MLCCs.
    for key, row, name, flds in (("r0603", "Chip resistor 0603 (1608 metric)", "Resistor 0603", [(F["res"], True)]),
                                 ("c0603", "Chip capacitor 0603 (1608 metric)", "Capacitor 0603", [(F["cap"], True), (F["volt"], False)]),
                                 ("c0805", "Chip capacitor 0805 (2012 metric)", "Capacitor 0805", [(F["cap"], True), (F["volt"], False)])):
        ids = from_template(s, row, name, {"skeleton": "two_terminal", "polarised": False})
        set_fields(s, ids[0], flds)
        L[key] = dict(zip(["component", "symbol", "package", "footprint", "binding"], ids))
        publish.append(ids)

    # Green 0603 LED: Kingbright APT1608SGC. The chip LED row is this
    # datasheet's: 1.6 x 0.8 x 0.75 body, 0.8 x 0.8 lands 0.85 apart, pad 1
    # the cathode (K), with the generated chip-LED model.
    ids = from_template(s, "Chip LED 0603 (1608 metric)", "APT1608SGC", {"skeleton": "two_terminal", "polarised": True},
                        symbol="led", prefix="D")
    L["led"] = dict(zip(["component", "symbol", "package", "footprint", "binding"], ids))
    publish.append(ids)

    # Omron B3U-1000P tactile switch (BOOT, RESET).
    ids = from_template(s, "Chip resistor 1206 (3216 metric)", "B3U-1000P", {"skeleton": "two_terminal", "polarised": False},
                        symbol="pushbutton", prefix="SW")
    comp, sym, pkg, fp, bind = ids
    replace_pads(s, fp, [([-1.7, 0.0], [0.8, 1.7]), ([1.7, 0.0], [0.8, 1.7])],
                 "Omron B3U datasheet p.2, PCB Pad (Top View), B3U-1000P: two 0.8 x 1.7 pads, 2.6 between, 4.2 overall")
    # The designator sits outside the body outline, on the -y side: the
    # resistor row's spot (+1.8) printed it across the outline's +y line.
    silk_half, silk_w, text_h, text_gap = 1.4, 0.12, 1.0, 0.15
    set_graphics(s, fp, extra=[rect([-1.5, -1.25], [1.5, 1.25], 0.1, "assembly"),
                               rect([-2.35, -1.5], [2.35, 1.5], 0.05, "courtyard"),
                               rect([-1.05, -silk_half], [1.05, silk_half], silk_w, "silkscreen")],
                 silk_text_at=[0.0, -round(silk_half + silk_w / 2 + text_gap + text_h / 2, 3)])
    generic_package(s, pkg, [3.0, 2.5, 1.6], "Omron B3U datasheet p.2, Dimensions (B3U-1000P): 3 x 2.5 body (4 over terminals), 1.6 high", "pa9t")
    L["sw"] = dict(zip(["component", "symbol", "package", "footprint", "binding"], ids))
    publish.append(ids)

    # Pin headers.
    for key, row, name in (("h12", "Pin header 1×12 2.54 mm", "Pin header 1x12 2.54 mm"),
                           ("h4", "Pin header 1×4 2.54 mm", "Pin header 1x4 2.54 mm")):
        ids = from_template(s, row, name, {"skeleton": "connector"}, prefix="J")
        L[key] = dict(zip(["component", "symbol", "package", "footprint", "binding"], ids))
        publish.append(ids)

    # TLV75733PDBVR 3.3 V 1 A LDO (module needs >= 0.6 A, datasheet Table 6-2).
    V = "SOT-23-5"
    ids = pinout(s, "TLV75733PDBVR", "tlv757p", V, 5, [
        pin("in", "1", "IN", "power_input", V, page=3, table="Table 4-1"),
        pin("gnd", "2", "GND", "power_input", V, page=3, table="Table 4-1"),
        pin("en", "3", "EN", "input", V, "digital", page=3, table="Table 4-1"),
        pin("nc", "4", "NC", None, V, cls="device_nc", page=3, table="Table 4-1"),
        pin("out", "5", "OUT", "power_output", V, page=3, table="Table 4-1")])
    fp = package_from(s, ids[2], "SOT-23-5")
    # The starter row's lands are estimates; use TI's DBV0005A example board layout.
    replace_pads(s, fp, [([-0.95, -1.3], [0.6, 1.1]), ([0.0, -1.3], [0.6, 1.1]), ([0.95, -1.3], [0.6, 1.1]),
                         ([0.95, 1.3], [0.6, 1.1]), ([-0.95, 1.3], [0.6, 1.1])],
                 "TI SBVS320 DBV0005A example board layout (p.35): 5x 0.6 x 1.1 pads, 0.95 pitch, 2.6 between row centres")
    L["ldo"] = dict(zip(["component", "symbol", "package", "binding"], ids), footprint=fp)
    publish.append(ids + [fp])

    # TPD2E009DBZR USB ESD (SOT-23-3).
    V = "SOT-23-3"
    ids = pinout(s, "TPD2E009DBZR", "tpd2e009", V, 3, [
        pin("dp", "1", "D+", "passive", V, "unspecified", page=3, table="Pin Functions"),
        pin("dn", "2", "D-", "passive", V, "unspecified", page=3, table="Pin Functions"),
        pin("gnd", "3", "GND", "power_input", V, "power", page=3, table="Pin Functions")])
    fp = package_from(s, ids[2], "SOT-23-3")
    replace_pads(s, fp, [([-0.95, -1.05], [0.6, 1.3]), ([0.95, -1.05], [0.6, 1.3]), ([0.0, 1.05], [0.6, 1.3])],
                 "TI TPD2E009 DBZ0003A example board layout: 3x 0.6 x 1.3 pads, 1.9 between pins 1 and 2, 2.1 between row centres")
    L["esd"] = dict(zip(["component", "symbol", "package", "binding"], ids), footprint=fp)
    publish.append(ids + [fp])

    # GCT USB4105-GF-A USB-C receptacle (USB 2.0, 16 contacts on 12 pads, shell).
    V = "USB4105"
    rows = [("a1b12", "A1B12", "GND"), ("a4b9", "A4B9", "VBUS"), ("b8", "B8", "SBU2"), ("a5", "A5", "CC1"),
            ("b7", "B7", "DN2"), ("a6", "A6", "DP1"), ("a7", "A7", "DN1"), ("b6", "B6", "DP2"),
            ("a8", "A8", "SBU1"), ("b5", "B5", "CC2"), ("b4a9", "B4A9", "VBUS"), ("b1a12", "B1A12", "GND"),
            ("shell", "S", "SHIELD")]
    ids = pinout(s, "USB4105-GF-A", "usb4105", V, 13,
                 [pin(r, n, name, "passive", V, "unspecified", page=1, table="Pin/Signal table") for r, n, name in rows])
    comp, sym, pkg, bind = ids
    # The USB-C receptacle row is this drawing's (GCT USB4105 rev B4 sheet 1):
    # its form gives the package the generated receptacle model, and its
    # lands the footprint (12 SMD lands, four plated shell slots, two NPTH
    # pegs), landing the pinout's terminals by number. Footprint origin: the
    # outline centre, mating face toward -y.
    fid = package_from(s, pkg, USB_C_ROW)
    L["usbc"] = {"component": comp, "symbol": sym, "package": pkg, "binding": bind, "footprint": fid}
    publish.append(ids + [fid])

    bad = check(s)
    if bad:
        raise SystemExit(f"blocking findings: {bad}")
    for group in publish:
        publish_and_close(s, group, "publish part " + drafts_name(s, group[0]))

    # Manufacturer Parts.
    P = []
    R, C3, C8 = L["r0603"], L["c0603"], L["c0805"]
    yag = ("Yageo RC_L series thick-film chip resistors datasheet", "https://www.yageo.com/upload/media/product/productsearch/datasheet/rchip/PYu-RC_Group_51_RoHS_L_12.pdf")
    for mpn, ohm in (("RC0603FR-075K1L", 5.1e3), ("RC0603FR-0710KL", 10e3), ("RC0603FR-07470RL", 470.0)):
        P.append(part(s, mpn, "Yageo", R["component"], R["binding"], [(F["res"], ohm, "exact_nominal")], *yag, desc="0603 1 % 0.1 W"))
    mur = "Murata GRM series chip monolithic ceramic capacitors, product page datasheet"
    for mpn, farad, volt, c, url in (
            ("GRM188R71C104KA01D", 100e-9, 16.0, C3, "https://www.murata.com/en-us/products/productdetail?partno=GRM188R71C104KA01%23"),
            ("GRM188R61C105KA93D", 1e-6, 16.0, C3, "https://www.murata.com/en-us/products/productdetail?partno=GRM188R61C105KA93%23"),
            ("GRM21BR61C106KE15L", 10e-6, 16.0, C8, "https://www.murata.com/en-us/products/productdetail?partno=GRM21BR61C106KE15%23"),
            ("GRM21BR61A226ME51L", 22e-6, 10.0, C8, "https://www.murata.com/en-us/products/productdetail?partno=GRM21BR61A226ME51%23")):
        P.append(part(s, mpn, "Murata", c["component"], c["binding"],
                      [(F["cap"], farad, "exact_nominal"), (F["volt"], volt, "minimum_rating")], mur, url))
    for key, mpn, man, ds in (("led", "APT1608SGC", "Kingbright", "apt1608"), ("sw", "B3U-1000P", "Omron", "b3u"),
                              ("ldo", "TLV75733PDBVR", "Texas Instruments", "tlv757p"),
                              ("esd", "TPD2E009DBZR", "Texas Instruments", "tpd2e009"),
                              ("usbc", "USB4105-GF-A", "GCT", "usb4105")):
        t, rev, url = DS[ds]
        P.append(part(s, mpn, man, L[key]["component"], L[key]["binding"], [], t, url, rev))
    for key, mpn in (("h12", "TSW-112-07-G-S"), ("h4", "TSW-104-07-G-S")):
        P.append(part(s, mpn, "Samtec", L[key]["component"], L[key]["binding"], [],
                      "Samtec TSW through-hole 2.54 mm terminal strip", "https://www.samtec.com/products/tsw"))
    P.append(part(s, "ESP32-S31-WROOM-3", "Espressif", 1, 4, [],
                  "ESP32-S31-WROOM-3 & WROOM-3U Datasheet", "https://documentation.espressif.com/esp32-s31-wroom-3_wroom-3u_datasheet_en.pdf", "v0.7"))
    publish_and_close(s, P, "publish Parts")
    L["parts"] = P
    return L
