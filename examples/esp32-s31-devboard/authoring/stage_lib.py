"""Stage 1: the parts library of the ESP32-S31 dev board, in the project."""
import json

DS = {
    "tlv757p": ("TLV757P 1-A Low-Dropout Regulator datasheet", "SBVS320", "https://www.ti.com/lit/ds/symlink/tlv757p.pdf"),
    "tpd2e009": ("TPD2E009 2-Channel ESD Protection for High-Speed Interfaces", "SLLSE91", "https://www.ti.com/lit/ds/symlink/tpd2e009.pdf"),
    "usb4105": ("GCT USB4105 USB Type-C receptacle for USB 2.0, drawing", "B4 (2023-12-18)", "https://gct.co/files/drawings/usb4105.pdf"),
    "b3u": ("Omron B3U ultra-small surface-mounting tactile switch", "catalog", "https://omronfs.omron.com/en_US/ecb/products/pdf/en-b3u.pdf"),
    "apt1608": ("Kingbright APT1608SGC 1.6 x 0.8 mm SMD chip LED", "V.22B (2023-12-07)", "https://www.kingbrightusa.com/images/catalog/SPEC/APT1608SGC.pdf"),
}


def meta(name, provenance, description=""):
    return {"name": name, "description": description, "provenance": provenance}


def drafts(s):
    """{id: draft} of the session's drafts."""
    r = s.raw("project.inspect")["result"]
    return {d["draft"]["id"] if "draft" in d else d["id"]: d["draft"] for d in r["drafts"]}


def edit(s, items, note):
    s.op("library.edit", {"edits": items}, note)


def pin(row, num, name, direction, variant, signal="power", cls="electrical", page=None, table=None):
    r = {"row": row, "number": {variant: num}, "name": name, "class": cls,
         "source": {k: v for k, v in (("page", page), ("table", table)) if v is not None}}
    if cls == "electrical":
        label = name.lower().replace("+", "p").replace("-", "n")
        r["functions"] = [{"label": label, "direction": direction, "signal": signal}]
        r["default"] = label
    return r


def pinout(s, name, ds, variant, count, pins):
    title, rev, url = DS[ds]
    doc = {"format": 1, "device": {"name": name, "reference": {"title": title, "document_revision": rev, "url": url}},
           "variants": [{"name": variant, "terminal_count": count}], "peripherals": [], "pins": pins, "rules": []}
    target = {"target": "new", "name": name}
    base = s.query("library.pinout_preview", {"target": target, "document": doc})["result"]["base"]
    r = s.op("library.pinout_apply", {"target": target, "document": doc, "base": base}, f"pinout {name}")
    return r["result"]["outcome"]["report"]["drafts"]  # component, symbol, package, binding


def from_template(s, row, name, skeleton, symbol=None, prefix=None):
    # This board types its own fields, so a chip row brings none of the standard ones.
    p = {"without_fields": True} if row.startswith("Chip ") else {}
    p |= {"row": {"source": "starter", "name": row}, "skeleton": skeleton, "name": name}
    if symbol:
        p["symbol"] = symbol
    if prefix:
        p["designator_prefix"] = prefix
    r = s.op("library.new_from_template", p, f"template {name}")
    return r["result"]["outcome"]["ids"]  # component, symbol, package, footprint, binding


def package_from(s, package, row):
    req = {"draft": package, "row": {"source": "starter", "name": row}}
    s.query("library.package_plan", {"request": req})
    r = s.op("library.package_from_template", req, f"package {row}")
    return r["result"]["outcome"]["plan"]["footprint"]


def check(s, ids=None):
    c = s.query("component.check_drafts", {})
    bad = []
    for d in c["result"]:
        if ids and d["target"]["id"] not in ids:
            continue
        for f in d["findings"]:
            if f["blocks"]:
                bad.append((d["target"]["id"], d["kind"], f["code"], f["message"][:200]))
    return bad




def rect(a, b, width, layer):
    return {"item": {"shape": {"fill": "none", "shape": {"rect": {"a": a, "b": b}}, "width": width}},
            "layer": layer, "side": "component"}






def generic_package(s, pkg_id, envelope, prov, material="epoxy_mould"):
    d = drafts(s)[pkg_id]
    body = d["definition"]["body"]
    body["envelope"] = envelope
    body["form"] = {"family": "generic"}
    body["model"] = {"source": "envelope", "material": material}
    edit(s, [{"id": pkg_id, "meta": d["meta"] | {"provenance": prov}, "definition": d["definition"]}], f"generic package {pkg_id}")

