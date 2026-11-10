"""Stage 3: stackup, board shape, mounting holes, placement, rules, pair."""
import json

W, H = 40.0, 64.0
PLACE = {  # designator: (x, y, rotation); J1's y puts its mating face on the bottom edge (see build)
    "J1": (20.0, None, 0), "U3": (20.0, 12.5, 180), "R4": (16.0, 11.5, 90), "R5": (24.0, 11.5, 90),
    "U2": (10.0, 13.0, 0), "C1": (7.0, 11.5, 90), "C2": (7.0, 15.5, 90),
    "R2": (33.0, 10.0, 0), "D1": (33.0, 12.5, 0),
    "U1": (20.0, 49.0, 0), "J2": (2.54, 21.0, 90), "J3": (37.46, 21.0, 90), "J4": (37.5, 52.0, 90),
    "SW1": (4.5, 42.0, 90), "SW2": (33.0, 57.0, 90),
    "R1": (6.6, 53.3, 270), "C4": (6.6, 50.3, 270), "C3": (6.6, 56.2, 90),
    "R3": (4.0, 50.0, 270), "D2": (4.0, 47.0, 90),
}
HOLES = [(3.5, 3.5), (36.5, 3.5), (3.5, 60.5), (36.5, 60.5)]
# The front face of the USB4105 part file's Shell body (parts/usb4105.solidcraft):
# its persistent Solidcraft name (the start face of the extruded shell
# profile), and a point on it below the opening.
USB_FRONT = ("Shell", "F2:start", [0.0, -3.675, 0.2])


def oid(r):
    o = r["result"]["outcome"]
    for k in ("class", "pair", "id", "group"):
        if k in o:
            return o[k]
    raise SystemExit(f"no id in {o}")


def nets_by_name(s):
    p = s.raw("project.inspect")["result"]["project"]
    return {n["name"]: n["id"] for n in p["nets"]}, p


def build(s, L, S):
    out = {}
    foil = lambda plane: {"thickness_um": 35.0, "plane": plane}
    pre = {"kind": "prepreg", "thickness_mm": 0.2, "glass": "style2116"}
    s.op("layout.set_stackup", {"copper": [foil(False), foil(True), foil(True), foil(False)],
                                "dielectrics": [pre, {"kind": "core", "thickness_mm": 1.2, "glass": "style7628"}, pre],
                                "mask_top": True, "mask_bottom": True, "silkscreen_top": True, "silkscreen_bottom": True},
         "4-layer stackup: signal / GND / power / signal")
    s.op("board.set_rectangle", {"size": [W, H]})
    s.op("board.start_sketch", None)
    # Mounting holes need sketch points; the created point ids are not
    # reported, so read them back from the stored sketch.
    s.op("board.edit", {"edits": [{"op": "sketch", "edit": {"add_point": {"at": list(h)}}} for h in HOLES]}, "hole centres")
    p = s.raw("project.inspect")["result"]["project"]
    sk = json.loads(p["board_shape"]["sketch"]["sketch"])
    pts = [pt["id"] for pt in sk["points"] if tuple(pt["pos"]) in HOLES]
    s.op("board.edit", {"edits": [{"op": "set_parameter", "param": {"name": "corner_r", "expr": "1.5 mm", "unit": "mm", "comment": "Board corner radius"}},
                                  {"op": "fillet", "corners": [], "radius": {"param": "corner_r"}}]
                        + [{"op": "add_mounting_hole", "center": pid, "drill": "2.7 mm", "plated": False} for pid in pts]},
         "rounded corners and M2.5 mounting holes")
    # Each hole sits `inset` from its two nearest edges, so the sketch is
    # fully defined and a width or height edit carries the holes along.
    edges = {e["name"]: e["curve"] for e in p["board_shape"]["sketch"]["edges"]}
    at = {pt["id"]: tuple(pt["pos"]) for pt in sk["points"]}
    to_edge = lambda pid, edge: {"op": "sketch", "edit": {"add_dimension": {
        "kind": "distance", "refs": [{"point": pid}, {"curve": edges[edge]}], "value": {"param": "inset"}}}}
    s.op("board.edit", {"edits": [{"op": "set_parameter", "param": {"name": "inset", "expr": "3.5 mm", "unit": "mm", "comment": "Mounting hole inset from the edges"}}]
                        + [to_edge(pid, "left" if at[pid][0] < W / 2 else "right") for pid in pts]
                        + [to_edge(pid, "bottom" if at[pid][1] < H / 2 else "top") for pid in pts]},
         "mounting holes inset from the edges")
    out["holes"] = pts

    # Fabrication profile: the built-in "Standard 6/6" (a generic estimate,
    # not a fabricator's data), previewed first.
    profiles = s.query("rules.profiles", {})["result"]
    profiles = profiles if isinstance(profiles, list) else profiles.get("profiles", [])
    std = next(p.get("profile", p) for p in profiles if p.get("profile", p).get("id") == "std-6-6")
    out["profile_preview"] = s.query("rules.preview_profile", {"profile": std})
    s.op("rules.set_profile", {"profile": std}, "profile Standard 6/6")
    # Manufacturing rules the default profile leaves at 0 (not checked):
    # copper and legend clearance to the board edge.
    for a in ("track", "fill", "pad", "via"):
        s.op("rules.set_override", {"key": {"rule": "clearance", "a": a, "b": "board_edge"},
                                    "value": {"kind": "length", "value": 0.3}}, f"{a} to edge 0.3 mm")
    s.op("rules.set_override", {"key": {"rule": "silk_edge_clearance"}, "value": {"kind": "length", "value": 0.2}},
         "legend to edge 0.2 mm")

    s.op("layout.sync", None, "update PCB from schematic")
    # The USB-C footprint's origin is the receptacle's outline centre with
    # the mating face toward -y, so half its depth puts the face on y = 0.
    lib = s.raw("project.inspect")["result"]["project"]["library"]
    usbc = next(k["definition"]["form"] for k in lib["packages"] if k["id"] == L["usbc"]["package"])
    place = dict(PLACE, J1=(PLACE["J1"][0], usbc["depth"] / 2, PLACE["J1"][2]))
    for des, (x, y, r) in place.items():
        inst = S["instances"][des]["id"]
        s.op("layout.move", {"instance": inst, "position": [x, y], "rotation_deg": float(r), "layer": "Top"}, f"place {des}")
    # J1 is held by its 3D model: the shell's front face flush with the
    # bottom edge, and its origin on the board's centre line, so it follows
    # a change of the board's width.
    p = s.raw("project.inspect")["result"]["project"]
    edges = {e["name"]: e["curve"] for e in p["board_shape"]["sketch"]["edges"]}
    j1 = S["instances"]["J1"]["id"]
    body, face, pick = USB_FRONT
    front = {"part": {"feature": "face", "placement": j1, "face": {"body": body, "face": face, "pick": pick}}}
    origin = {"part": {"feature": "origin", "target": {"kind": "placement", "id": j1}}}
    for refs, value, note in (([front, {"board": {"curve": edges["bottom"]}}], {"fixed": 0.0}, "J1 flush with the bottom edge"),
                              ([origin, {"board": {"curve": edges["left"]}}], {"expr": "width / 2"}, "J1 on the centre line")):
        s.apply({"editor": "Dimension", "operation": {"command": "layout.dimension_add", "params": {
            "dimension": {"kind": {"dim": "distance"}, "refs": refs, "value": value}}}}, note)
    # The ESD array and the CC pull-downs sit at fixed offsets from J1, so
    # the receptacle's cluster moves as one when J1 does, and its routes
    # with it (`layout::dimensions::carry`).
    at_j1 = place["J1"]
    for des in ("U3", "R4", "R5"):
        part = {"part": {"feature": "origin", "target": {"kind": "placement", "id": S["instances"][des]["id"]}}}
        x, y, _ = place[des]
        for kind, value in (("horizontal_distance", abs(x - at_j1[0])), ("vertical_distance", abs(y - at_j1[1]))):
            s.apply({"editor": "Dimension", "operation": {"command": "layout.dimension_add", "params": {
                "dimension": {"kind": {"dim": kind}, "refs": [origin, part], "value": {"fixed": round(value, 6)}}}}},
                    f"{des} {kind.split('_')[0]} from J1")

    nets, p = nets_by_name(s)
    out["nets"] = nets
    # Net classes.
    r = s.op("net_class.add", {"class": {"name": "Power", "track_width": 0.5, "track_width_min": 0.3,
                                         "via_drill": 0.4, "via_diameter": 0.8}}, "Power class")
    power = oid(r)
    s.op("net_class.assign", {"class": power, "nets": [nets["GND"], nets["+3V3"], nets["VBUS"]]}, "assign Power")
    r = s.op("net_class.add", {"class": {"name": "USB", "track_width_min": 0.12, "via_drill": 0.3, "via_diameter": 0.6,
                                         "signal": {"diff_impedance": 90.0, "impedance_tolerance": 10.0,
                                                    "pair_max_skew": 0.15, "pair_max_uncoupled": 12.0}}}, "USB class")
    usb = oid(r)
    s.op("net_class.assign", {"class": usb, "nets": [nets["USB_DP"], nets["USB_DN"]]}, "assign USB")
    sugg = s.query("diff_pair.suggest", {})
    out["pair_suggestions"] = sugg
    r = s.op("diff_pair.add", {"pair": {"name": "USB", "positive": nets["USB_DP"], "negative": nets["USB_DN"]}}, "USB pair")
    out["pair"] = oid(r)
    out["classes"] = {"power": power, "usb": usb}
    out["impedance"] = s.query("impedance.table", {})
    return out
