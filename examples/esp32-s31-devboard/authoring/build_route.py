"""Stage 4: routing, the USB pair, fills."""
import json
from geom import pads as world_pads

TOP, BOT = "Top", "Bottom"
SIG, PWR, USBW = 0.25, 0.5, 0.3


class Router:
    def __init__(self, s, nets):
        self.s, self.nets = s, nets
        self.short = []

    def path(self, net, pts, width=SIG, layer=TOP, mode="walkaround", note=""):
        n = self.nets[net]
        if net in ("GND", "+3V3", "VBUS"):
            width = max(width, 0.3)   # the Power class minimum
        for a, b in zip(pts, pts[1:]):
            r = self.s.op("layout.route_step", {"request": {"net": n, "width": width, "layer": layer, "head": list(a), "to": list(b),
                                                           "posture": "free", "mode": mode}},
                          f"route {net} {note} {a}->{b}", ok_required=False)
            if not r.get("ok"):
                self.short.append((net, note, a, b, r["error"]["message"][:300]))
                return False
        return True

    def via(self, net, at, drill=0.3, dia=0.6):
        r = self.s.op("layout.add_via", {"via": {"net": self.nets[net], "position": list(at), "drill": drill, "diameter": dia,
                                                "span": "through", "tented": True}}, f"via {net} {at}", ok_required=False)
        if not r.get("ok"):
            self.short.append((net, "via", at, None, r["error"]["message"][:300]))
            return False
        return True

    def drop(self, net, pad, at, width=SIG):
        """A short top stub from a pad to a new via."""
        if self.path(net, [pad, at], width, note="fanout"):
            self.via(net, at)


def build(s, L, S, PCB):
    P = s.raw("project.inspect")["result"]["project"]
    nets = {n["name"]: n["id"] for n in P["nets"]}
    pp = {}
    for des, num, pos, net, pad, rot in world_pads(P):
        pp.setdefault((des, num), []).append(pos)
    pad = lambda des, num, i=0: pp[(des, num)][i]
    R = Router(s, nets)

    # --- USB connector: join the two plug orientations ------------------
    R.path("USB_DP", [pad("J1", "A6"), (19.75, 8.6), (20.75, 8.6)], USBW, note="A6 to B6")
    R.path("USB_DP", [pad("J1", "B6"), (20.75, 12.0), pad("U3", "1")], USBW, note="B6 to ESD")
    R.path("USB_DN", [pad("J1", "B7"), (19.25, 12.0), pad("U3", "2")], USBW, note="B7 to ESD")
    R.drop("USB_DN", pad("J1", "A7"), (20.25, 6.0), USBW)
    R.drop("USB_DN", pad("J1", "B7"), (19.25, 6.0), USBW)
    R.path("USB_DN", [(20.25, 6.0), (19.25, 6.0)], USBW, BOT, note="A7 jumper")
    R.drop("GND", pad("U3", "3"), (20.0, 10.5))
    # CC pull-downs.
    R.path("CC1", [pad("J1", "A5"), (18.75, 9.8), (16.0, 9.8), pad("R4", "1")], SIG)
    R.path("CC2", [pad("J1", "B5"), (21.75, 9.8), (24.0, 9.8), pad("R5", "1")], SIG)
    R.drop("GND", pad("R4", "2"), (16.0, 13.0))
    R.drop("GND", pad("R5", "2"), (24.0, 13.0))
    # GND and VBUS pads of the receptacle.
    # The receptacle's GND pads join the shell pads beside them (plated
    # through, so on every plane).
    R.path("GND", [pad("J1", "A1B12"), pad("J1", "S", 0)], 0.4, note="A1B12 to shell")
    R.path("GND", [pad("J1", "B1A12"), pad("J1", "S", 2)], 0.4, note="B1A12 to shell")
    R.drop("VBUS", pad("J1", "A4B9"), (17.6, 9.0), 0.4)
    R.drop("VBUS", pad("J1", "B4A9"), (22.4, 9.0), 0.4)
    R.path("VBUS", [(22.4, 9.0), (17.6, 9.0)], PWR, BOT, note="B4A9 to A4B9")

    # --- LDO -------------------------------------------------------------
    R.path("VBUS", [(17.6, 9.0), (11.6, 9.0), (10.95, 9.65), pad("U2", "3")], PWR, note="to LDO EN")
    R.path("VBUS", [(10.95, 10.6), (9.05, 10.6), pad("U2", "1")], PWR, note="to LDO IN")
    R.path("VBUS", [(9.05, 10.6), pad("C1", "1")], PWR, note="to C1")
    R.path("VBUS", [pad("C1", "1"), (5.6, 9.35), (5.6, 7.03), pad("J2", "1")], PWR, note="to J2.1")
    R.path("GND", [pad("U2", "2"), (10.0, 13.0)], 0.3, note="LDO GND")
    R.via("GND", (10.0, 13.0))
    R.drop("GND", pad("C1", "2"), (7.0, 13.15), 0.4)
    R.drop("GND", pad("C2", "2"), (8.0, 16.6), 0.4)
    R.path("+3V3", [pad("U2", "5"), pad("C2", "1")], PWR, note="LDO OUT to C2")
    R.drop("+3V3", pad("U2", "5"), (8.6, 15.5), 0.4)

    # --- LEDs ---------------------------------------------------------------
    R.path("LED_PWR", [pad("R2", "2"), pad("D1", "2")], SIG)
    R.drop("+3V3", pad("R2", "1"), (27.3, 10.0))
    R.drop("GND", pad("D1", "1"), (27.2, 12.5))
    R.drop("GND", pad("D2", "1"), (2.0, 46.175))
    R.path("LED_USR_A", [pad("R3", "2"), pad("D2", "2")], SIG)
    R.drop("LED_USR", pad("U1", "6"), (8.4, 51.95))
    R.path("LED_USR", [(8.4, 51.95), (4.0, 52.0)], SIG, BOT, note="under EN")
    R.via("LED_USR", (4.0, 52.0))
    R.path("LED_USR", [(4.0, 52.0), pad("R3", "1")], SIG)

    # --- EN, BOOT, UART ---------------------------------------------------
    R.path("EN", [pad("U1", "5"), pad("R1", "2"), pad("C4", "1"), (5.4, 50.925), (5.4, 45.5), (6.2, 44.7), (6.2, 40.3),
                  pad("SW1", "1")], SIG)
    R.drop("GND", pad("SW1", "2"), (2.0, 43.7))
    R.drop("GND", pad("C4", "2"), (7.7, 49.675))
    R.path("BOOT", [pad("U1", "71"), pad("SW2", "1")], SIG)
    R.drop("GND", pad("SW2", "2"), (34.3, 58.7))
    R.path("TX0", [pad("U1", "68"), (32.0, 52.8), (34.07, 50.73), pad("J4", "2")], SIG)
    R.path("RX0", [pad("U1", "69"), (35.0, 53.65), (35.38, 53.27), pad("J4", "3")], SIG)

    # --- 3V3 at the module --------------------------------------------------
    R.path("+3V3", [pad("U1", "4"), pad("U1", "3"), (6.6, 54.5), pad("C3", "1")], 0.4, note="module 3V3")
    R.path("+3V3", [(6.6, 54.5), pad("R1", "1")], 0.4)
    R.path("+3V3", [(8.0, 54.5), (8.0, 55.25)], 0.4)
    R.via("+3V3", (8.0, 55.25))

    # --- module GND pins -----------------------------------------------------
    R.path("GND", [pad("U1", "2"), pad("U1", "1"), pad("U1", "99"), pad("U1", "94"), pad("U1", "93"), pad("U1", "92")], 0.3, note="top-left GND row")
    R.path("GND", [pad("U1", "1"), (8.5, 56.2), (7.9, 56.825), pad("C3", "2")], 0.3, note="C3 GND")
    R.drop("GND", pad("U1", "93"), (11.9, 56.3), 0.3)
    R.path("GND", [pad("U1", "90"), pad("U1", "73"), pad("U1", "98"), pad("U1", "72")], 0.3, note="top GND row")
    for p in ("88", "81", "74"):
        R.drop("GND", pad("U1", p), (pad("U1", p)[0], 56.3), 0.3)
    R.path("GND", [pad("U1", "96"), pad("U1", "26")], 0.3, note="bottom-left GND")
    R.drop("GND", pad("U1", "26"), (11.075, 36.0), 0.3)
    R.path("GND", [pad("U1", "47"), pad("U1", "97"), pad("U1", "48")], 0.3, note="bottom-right GND")
    R.drop("GND", pad("U1", "47"), (28.925, 36.0), 0.3)
    for i in range(9):
        R.via("GND", pad("U1", "95", i))

    # --- GPIO buses to the headers (bottom layer) ----------------------------
    for k in range(9):            # IO4 (pin 10) .. IO12 (pin 18) -> J2.12 .. J2.4
        mp = str(10 + k)
        y = pad("U1", mp)[1]
        vx = 8.3 if k % 2 == 0 else 7.5
        net = f"IO{4 + k}"
        R.drop(net, pad("U1", mp), (vx, y), 0.2)
        xc = 3.7 + 0.4 * k
        hp = pad("J2", str(12 - k))
        R.path(net, [(vx, y), (xc, y), (xc, hp[1] + 0.6), (hp[0] + 0.6, hp[1]), hp], 0.2, BOT, note="bus")
    for k in range(11):           # IO52 (pin 62) .. IO42 (pin 52) -> J3.12 .. J3.2
        mp = str(62 - k)
        y = pad("U1", mp)[1]
        net = f"IO{52 - k}"
        R.drop(net, pad("U1", mp), (31.7, y), 0.2)
        xc = 36.3 - 0.4 * k
        hp = pad("J3", str(12 - k))
        R.path(net, [(31.7, y), (xc, y), (xc, hp[1] + 0.6), (hp[0] - 0.6, hp[1]), hp], 0.2, BOT, note="bus")

    # --- USB pair from the ESD diodes to the module ---------------------------
    pair = PCB["pair"]
    r = s.op("layout.route_pair", {"request": {"pair": pair, "start": [pad("U3", "1"), pad("U3", "2")],
                                               "legs": [{"layer": TOP, "points": [[20.0, 14.8], [20.0, 27.0], [25.1, 32.1], [25.1, 33.0]]}],
                                               "end": [pad("U1", "43"), pad("U1", "42")]}}, "USB pair", ok_required=False)
    if not r.get("ok"):
        R.short.append(("USB pair", "route_pair", None, None, r["error"]["message"][:500]))

    # --- fills ----------------------------------------------------------------
    W, H = 40.0, 64.0
    rect = {"start": [0.0, 0.0], "segments": [{"line": {"to": [W, 0.0]}}, {"line": {"to": [W, H]}},
                                              {"line": {"to": [0.0, H]}}, {"line": {"to": [0.0, 0.0]}}]}
    for name, net, layer in (("GND plane", "GND", {"Inner": 1}), ("3V3 plane", "+3V3", {"Inner": 2}),
                             ("GND bottom", "GND", BOT), ("GND top", "GND", TOP)):
        spec = {"name": name, "net": nets[net], "layer": layer, "outline": rect}
        if layer == TOP:
            # Keep the top pour well away from the USB pair (and everything
            # else on top) so the pair stays a microstrip, not a coplanar line.
            spec["settings"] = {"clearance": 0.4}
        s.op("fill.add", {"fill": spec}, f"fill {name}")
    s.op("fill.refill", {"target": {"fills": "all"}}, "refill")
    return R.short
