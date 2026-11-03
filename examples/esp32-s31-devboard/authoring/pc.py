"""A small JSON-lines client for `pcbcraft-cli stdio` that records every
request it sends, so the session can be replayed exactly."""
import json, subprocess, sys

import os as _os
CLI = _os.environ.get("PCBCRAFT_CLI", "target/nix/ci/pcbcraft-cli")


class Refused(Exception):
    pass


class Session:
    def __init__(self, record=None, start=None):
        args = [CLI, "stdio"] + ([start] if start else [])
        import os
        env = dict(os.environ)
        import tempfile
        base = os.environ.get("E2E_DIRS") or tempfile.mkdtemp(prefix="pcbcraft-e2e-")
        env["PCBCRAFT_CONFIG_DIR"] = base + "/config"
        env["PCBCRAFT_CACHE_DIR"] = base + "/cache"
        self.p = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1, env=env)
        self.record = open(record, "w") if record else None
        self.rev = None
        self.log = []

    def raw(self, command, params=None, note=None, ok_required=True):
        req = {"command": command, "params": params if params is not None else {}}
        line = json.dumps(req, separators=(",", ":"))
        self.p.stdin.write(line + "\n")
        self.p.stdin.flush()
        reply = json.loads(self.p.stdout.readline())
        if reply.get("ok"):
            if self.record:
                self.record.write(line + "\n")
                self.record.flush()
        else:
            self.log.append((command, note, reply.get("error")))
            if ok_required:
                raise Refused(f"{command} ({note}): {json.dumps(reply.get('error'))[:3000]}")
        return reply

    def revision(self):
        r = self.raw("project.inspect")
        return r["result"]["revision"] if "revision" in r["result"] else r["result"]["document"]["revision"]

    def apply(self, op, note=None, ok_required=True):
        if self.rev is None:
            self.rev = self.revision()
        r = self.raw("project.apply", {"expected_revision": self.rev, "operation": op}, note, ok_required)
        if r.get("ok"):
            res = r["result"]
            self.rev = res.get("revision", self.rev)
        return r

    def op(self, command, params, note=None, ok_required=True):
        editor = {"library": "Library", "component": "Instance", "instance": "Instance", "symbol": "Schematic",
                  "schematic": "Schematic", "label": "Schematic", "sheet": "Schematic", "sheet_symbol": "Schematic",
                  "sheet_pin": "Schematic", "drawing": "Schematic", "project": "Schematic", "layout": "Layout",
                  "rules": "Rules", "net_class": "Rules", "diff_pair": "Rules", "length_group": "Rules",
                  "fill": "Fill", "keepout": "Fill", "board": "Board", "fabrication": "Fabrication"}[command.split(".")[0]]
        body = {"command": command}
        if params is not None:
            body["params"] = params
        return self.apply({"editor": editor, "operation": body}, note or command, ok_required)

    def query(self, query, params=None):
        q = {"query": query}
        if params is not None:
            q["params"] = params
        r = self.raw("project.query", q)
        return r["result"]
