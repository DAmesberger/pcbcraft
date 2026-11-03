"""World pad positions and nets of a project (from project.inspect JSON)."""
import math


def xf(pt, pos, rot, bottom=False):
    x, y = pt
    if bottom:
        x = -x
    c, s = math.cos(math.radians(rot)), math.sin(math.radians(rot))
    return (round(pos[0] + c * x - s * y, 4), round(pos[1] + s * x + c * y, 4))


def pads(P):
    """[(designator, pad number, (x, y), net name or None, pad dict, rotation)]"""
    lib = P["library"]
    fps = {(f["id"], f["revision"]): f["definition"] for f in lib["footprints"]}
    binds = {(b["id"], b["revision"]): b["definition"] for b in lib["bindings"]}
    comps = {(c["id"], c["revision"]): c["definition"] for c in lib["components"]}
    net_of = {}
    for n in P["nets"]:
        for t in n["terminals"]:
            net_of[(t["instance"], t["terminal"])] = n["name"]
    place = {p["instance"]: p for p in P["placements"]}
    out = []
    for inst in P["instances"]:
        pl = place.get(inst["id"])
        if not pl:
            continue
        comp = comps[(inst["component"]["id"], inst["component"]["revision"])]
        bref = next(i for i in comp["implementations"] if i["id"] == inst["physical"]["binding"])
        b = binds[(bref["id"], bref["revision"])]
        fpref = next(f for f in b["footprints"] if f["id"] == inst["physical"]["footprint"])
        fp = fps[(fpref["id"], fpref["revision"])]
        pk2t = {m["package_terminal"]: m["terminal"] for m in b["mappings"]}
        for pad in fp["pads"]:
            pos = xf(pad["position"], pl["position"], pl["rotation_deg"], pl["layer"] == "Bottom")
            term = pk2t.get(pad.get("terminal"))
            out.append((inst["designator"], pad["number"], pos, net_of.get((inst["id"], term)), pad, pl["rotation_deg"]))
    return out
