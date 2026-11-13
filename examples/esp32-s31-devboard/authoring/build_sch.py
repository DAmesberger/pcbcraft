"""Stage 2: schematic capture on two sheets with power ports and labels."""
import json

POWER = {"GND": "ground", "+3V3": "bar", "VBUS": "arrow"}
GLOBAL = {"USB_DP", "USB_DN"}


class Lib:
    def __init__(self, s):
        p = s.raw("project.inspect")["result"]["project"]
        self.lib = p["library"]
        self.by = {(k, d["id"]): d for k in ("components", "symbols", "packages", "bindings", "footprints") for d in self.lib[k]}

    def terminals(self, comp, binding):
        """number -> component terminal id, and name -> [ids]."""
        b = self.by[("bindings", binding)]["definition"]
        pkg = self.by[("packages", b["package"]["id"])]["definition"]
        des = {t["id"]: t["designation"] for t in pkg["terminals"]}
        by_num = {des[m["package_terminal"]]: m["terminal"] for m in b["mappings"]}
        c = self.by[("components", comp)]["definition"]
        by_name = {}
        for t in c["terminals"]:
            by_name.setdefault(t["name"], []).append(t["id"])
        return by_num, by_name

    def pins(self, symbol):
        return {p["terminal"]: p for p in self.by[("symbols", symbol)]["definition"]["units"][0]["pins"]}


def build(s, L):
    lib = Lib(s)
    out = {"instances": {}}
    s.op("project.set_properties", {"properties": {"title": "ESP32-S31 dev board", "revision": "A", "date": "2026-11-03",
                                                   "author": "Sparksmith e2e run (agent)", "company": "Sparksmith example"}})
    s.op("sheet.rename", {"sheet": 1, "name": "Power and USB"})
    r = s.op("sheet.add", {"name": "Module and IO", "paper": "a3", "orientation": "landscape"})
    sheet2 = r["result"]["outcome"].get("sheet") or r["result"]["outcome"].get("id")
    out["sheet2"] = sheet2

    F = L["fields"]
    def place(key, des, at, sheet, values=()):
        part = L[key]
        props = [{"field": F[f], "input": {"input": "text", "value": v}} for f, v in values]
        p = {"component": {"id": part["component"], "revision": 1}, "designator": des, "properties": props,
             "representation": part["symbol"], "unit": 1, "position": at,
             "physical": {"binding": part["binding"], "footprint": part["footprint"]}, "sheet": sheet}
        r = s.op("component.place", p, f"place {des}")
        oc = r["result"]["outcome"]
        inst = oc.get("instance") or oc.get("id")
        num, name = lib.terminals(part["component"], part["binding"])
        out["instances"][des] = {"id": inst, "key": key, "at": at, "sheet": sheet, "num": num, "name": name,
                                 "pins": lib.pins(part["symbol"])}
        return inst

    # Sheet 1: USB-C, ESD, CC pull-downs, LDO, bulk capacitors, power LED.
    place("usbc", "J1", [20.0, -50.0], 1)
    place("esd", "U3", [80.0, -50.0], 1)
    place("r0603", "R4", [20.0, -90.0], 1, [("res", "5.1k")])
    place("r0603", "R5", [20.0, -110.0], 1, [("res", "5.1k")])
    place("ldo", "U2", [150.0, -50.0], 1)
    place("c0805", "C1", [120.0, -80.0], 1, [("cap", "10u"), ("volt", "16V")])
    place("c0805", "C2", [180.0, -80.0], 1, [("cap", "22u"), ("volt", "10V")])
    place("r0603", "R2", [220.0, -50.0], 1, [("res", "470")])
    place("led", "D1", [220.0, -80.0], 1)
    # Sheet 2: module, EN RC and buttons, user LED, decoupling, headers.
    place("module", "U1", [110.0, -120.0], sheet2)
    place("c0603", "C3", [40.0, -60.0], sheet2, [("cap", "100n"), ("volt", "16V")])
    place("r0603", "R1", [200.0, -60.0], sheet2, [("res", "10k")])
    place("c0603", "C4", [200.0, -80.0], sheet2, [("cap", "1u"), ("volt", "16V")])
    place("sw", "SW1", [200.0, -100.0], sheet2)
    place("sw", "SW2", [200.0, -130.0], sheet2)
    place("r0603", "R3", [200.0, -160.0], sheet2, [("res", "470")])
    place("led", "D2", [200.0, -180.0], sheet2)
    place("h12", "J2", [290.0, -60.0], sheet2)
    place("h12", "J3", [340.0, -60.0], sheet2)
    place("h4", "J4", [290.0, -140.0], sheet2)

    def term(des, t):
        i = out["instances"][des]
        if t in i["num"]:
            return i["num"][t]
        ids = i["name"][t]
        if len(ids) != 1:
            raise SystemExit(f"{des} {t} is ambiguous: {ids}")
        return ids[0]

    def label(net, des, t):
        i = out["instances"][des]
        tid = term(des, t)
        pin = i["pins"][tid]
        x, y = i["at"][0] + pin["position"][0], i["at"][1] + pin["position"][1]
        dx, dy = {"right": (-5, 0), "left": (5, 0), "up": (0, -5), "down": (0, 5)}[pin["direction"]]
        if net in POWER:
            kind = {"type": "power", "style": POWER[net]}
            if net == "GND":
                dx, dy = (dx, dy) if dy else (dx, -3)
        elif net in GLOBAL:
            kind = {"type": "global", "shape": "bidirectional"}
        else:
            kind = {"type": "local"}
        s.op("label.place", {"sheet": i["sheet"], "name": net, "kind": kind, "position": [x + dx, y + dy],
                             "attach": {"instance": i["id"], "terminal": tid}}, f"label {net} at {des}.{t}")

    nets = {
        "VBUS": [("J1", "A4B9"), ("J1", "B4A9"), ("U3", None), ("U2", "IN"), ("U2", "EN"), ("C1", "1"), ("J2", "1")],
        "GND": [("J1", "A1B12"), ("J1", "B1A12"), ("J1", "S"), ("U3", "GND"), ("U2", "GND"), ("C1", "2"), ("C2", "2"),
                ("R4", "2"), ("R5", "2"), ("D1", "K"), ("C3", "2"), ("C4", "2"), ("SW1", "2"), ("SW2", "2"), ("D2", "K"),
                ("J2", "3"), ("J3", "1"), ("J4", "1")],
        "+3V3": [("U2", "OUT"), ("C2", "1"), ("R2", "1"), ("C3", "1"), ("R1", "1"), ("U1", "3"), ("U1", "4"), ("J2", "2"), ("J4", "4")],
        "CC1": [("J1", "A5"), ("R4", "1")],
        "CC2": [("J1", "B5"), ("R5", "1")],
        "USB_DP": [("J1", "A6"), ("J1", "B6"), ("U3", "D+"), ("U1", "43")],
        "USB_DN": [("J1", "A7"), ("J1", "B7"), ("U3", "D-"), ("U1", "42")],
        "LED_PWR": [("R2", "2"), ("D1", "A")],
        "EN": [("U1", "5"), ("R1", "2"), ("C4", "1"), ("SW1", "1")],
        "BOOT": [("U1", "71"), ("SW2", "1")],
        "LED_USR": [("U1", "6"), ("R3", "1")],
        "LED_USR_A": [("R3", "2"), ("D2", "A")],
        "TX0": [("U1", "68"), ("J4", "2")],
        "RX0": [("U1", "69"), ("J4", "3")],
    }
    nets["VBUS"] = [p for p in nets["VBUS"] if p[1]]
    gnd_pins = [1, 2, 26, 47, 48] + list(range(72, 91)) + list(range(92, 100))
    nets["GND"] += [("U1", str(n)) for n in gnd_pins]
    # Header order follows the module pins so the bus routes without crossings:
    # J2 pin 4..12 carries IO12..IO4 (module pins 18..10), J3 pin 2..12 IO42..IO52.
    j2 = {hp: 22 - hp for hp in range(4, 13)}
    j3 = {hp: 50 + hp for hp in range(2, 13)}
    for hp, mp in list(j2.items()) + list(j3.items()):
        h = "J2" if (hp, mp) in j2.items() else "J3"
        io = next(n for n in out["instances"]["U1"]["name"] if out["instances"]["U1"]["name"][n] == [term("U1", str(mp))])
        nets[io] = [("U1", str(mp)), (h, str(hp))]
    for net, ends in nets.items():
        for des, t in ends:
            label(net, des, t)
    out["nets"] = {k: v for k, v in nets.items()}

    # Name the nets after their labels (labels already name them) and mark
    # unused pins as no-connect: the module's unused IOs, the SBU pins.
    used = {(d, term(d, t)) for ends in nets.values() for d, t in ends}
    comp = lib.by[("components", L["module"]["component"])]["definition"]
    nc = []
    for t in comp["terminals"]:
        if ("U1", t["id"]) in used:
            continue
        if "electrical" in t["class"]:
            nc.append(t["id"])
    s.op("instance.set_no_connect", {"instance": out["instances"]["U1"]["id"], "terminals": nc, "no_connect": True}, "U1 unused pins")
    s.op("instance.set_no_connect", {"instance": out["instances"]["J1"]["id"], "terminals": [term("J1", "A8"), term("J1", "B8")],
                                     "no_connect": True}, "J1 SBU pins")
    out["no_connect_U1"] = len(nc)

    # Purchasing: resolve each group to a Part found by the group's query.
    groups = s.query("library.resolve_groups", {})
    out["groups"] = []
    for g in groups["result"]["groups"]:
        hits = s.query("library.search", {"query": g["query"], "limit": 100})["result"]["results"]
        parts = [h["entry"] for h in hits if h["entry"]["kind"] == "part"]
        out["groups"].append({"query": g["query"], "designators": g["designators"], "hits": [h["mpn"] for h in parts]})
        if parts:
            ref = {"id": parts[0]["lineage"], "revision": parts[0]["revision"]}
            s.op("instance.assign_part", {"instances": g["instances"], "part": ref}, f"assign {g['query']}")
    for v in out["instances"].values():
        v.pop("pins")
        v["num"] = {k: v2 for k, v2 in v["num"].items()}
    return out
