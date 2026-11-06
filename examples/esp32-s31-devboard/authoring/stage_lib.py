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


def text(at, content, height, layer="silkscreen"):
    return {"item": {"text": {"at": at, "content": content, "height": height,
                              "justify": {"h": "center", "v": "middle"}, "line_spacing": 1.6,
                              "rotation": "r0", "visible": True}}, "layer": layer, "side": "component"}


def rect(a, b, width, layer):
    return {"item": {"shape": {"fill": "none", "shape": {"rect": {"a": a, "b": b}}, "width": width}},
            "layer": layer, "side": "component"}


def smd(num, at, size, terminal):
    return {"kind": "smd", "number": num, "position": at, "rotation_deg": 0.0, "side": "top",
            "shape": {"rect": {"size": size}}, "mask": "inherit", "paste": "inherit", "terminal": terminal,
            "hole": None, "vias": []}


def footprint_body(package_id, pads, graphics):
    out_pads, out_gfx, n = [], [], 1
    for p in pads:
        out_pads.append(dict(p, id=n)); n += 1
    for g in graphics:
        out_gfx.append(dict(g, id=n)); n += 1
    return {"datum": {"offset": [0.0, 0.0], "rotation_deg": 0.0},
            "defaults": {"mask_expansion_mm": 0.05, "paste_reduction_mm": 0.0},
            "graphics": out_gfx, "next_local": n, "package": {"id": package_id, "revision": 1},
            "pads": out_pads, "polarity": None, "source": None}


def generic_package(s, pkg_id, envelope, prov, material="epoxy_mould", offset=None):
    d = drafts(s)[pkg_id]
    body = d["definition"]["body"]
    body["envelope"] = envelope
    body["form"] = {"family": "generic"}
    body["model"] = {"source": "envelope", "material": material}
    if offset:
        body["placement"]["translation"] = offset
    edit(s, [{"id": pkg_id, "meta": d["meta"] | {"provenance": prov}, "definition": d["definition"]}], f"generic package {pkg_id}")


def custom_footprint(s, name, pkg_id, binding_id, pads_by_terminal_number, graphics, prov):
    """A new footprint draft whose pads land package terminals by number."""
    d = drafts(s)
    terms = {t["designation"]: t["id"] for t in d[pkg_id]["definition"]["body"]["terminals"]}
    pads = []
    for p in pads_by_terminal_number:
        p = dict(p)
        num = p.pop("term")
        if num is not None:
            p["terminal"] = terms[num]
        pads.append(p)
    body = footprint_body(pkg_id, pads, graphics)
    r = s.op("library.new_draft", {"meta": meta(name, prov), "definition": {"kind": "footprint", "body": body}}, f"footprint {name}")
    fid = r["result"]["outcome"].get("id") or r["result"]["outcome"].get("ids", [None])[0]
    b = drafts(s)[binding_id]
    b["definition"]["body"]["footprints"] = [{"id": fid, "revision": 1}]
    edit(s, [{"id": binding_id, "meta": b["meta"], "definition": b["definition"]}], f"bind footprint {name}")
    return fid
