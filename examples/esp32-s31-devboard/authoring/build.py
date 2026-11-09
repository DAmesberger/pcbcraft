import json, sys, os, traceback
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from pc import Session, Refused
import build_lib

WT = os.getcwd()  # run from the repository root
MODULE = "examples/esp32-s31-wroom-3/esp32-s31-wroom-3.pcbcraft"
upto = sys.argv[1] if len(sys.argv) > 1 else "lib"
out = os.environ.get("E2E_OUT", "target/e2e-devboard")
os.makedirs(out, exist_ok=True)

s = Session(record=out + "/replay.jsonl")
s.raw("project.open", {"expected_revision": 0, "path": MODULE})
s.rev = None
stages = ["lib", "sch", "pcb", "route", "fab"]
state = {}
try:
    L = build_lib.build(s)
    state["lib"] = L
    json.dump(L, open(out + "/lib.json", "w"), indent=1)
    if upto != "lib":
        import build_sch
        state["sch"] = build_sch.build(s, L)
        json.dump(state["sch"], open(out + "/sch.json", "w"), indent=1)
    if upto in ("pcb", "route", "fab"):
        import build_pcb
        state["pcb"] = build_pcb.build(s, L, state["sch"])
        json.dump(state["pcb"], open(out + "/pcb.json", "w"), indent=1)
    if upto in ("route", "fab"):
        import build_route
        short = build_route.build(s, L, state["sch"], state["pcb"])
        s.op("project.rename", {"name": "ESP32-S31 dev board"})
        json.dump(short, open(out + "/route-short.json", "w"), indent=1)
        for x in short: print("SHORT", x)
except Refused as e:
    print("REFUSED:", e)
    traceback.print_exc(limit=3)
path = "examples/esp32-s31-devboard/esp32-s31-devboard.pcbcraft" if upto == "route" else out + f"/stage-{upto}.pcbcraft"
if os.path.exists(path):
    os.remove(path)
s.raw("project.save", {"path": path})
print("saved", path, "revision", s.rev)
