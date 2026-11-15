"""Fail if a wasm module has any import, or lacks a `run` export. Run with `python3 -I ci/wasm_imports.py <file.wasm>`.

A crate that builds for `wasm32v1-none` with zero imports cannot reach the host at all, so it is already a
valid sandboxed guest (ADR 0002). Exit codes: 0 clean, 1 at least one import, 2 no `run` export, 3 unparsable.
"""
import sys


class Reader:
    def __init__(self, data):
        self.b = data
        self.i = 0

    def byte(self):
        x = self.b[self.i]
        self.i += 1
        return x

    def leb(self):
        result = shift = 0
        while True:
            x = self.byte()
            result |= (x & 0x7F) << shift
            shift += 7
            if not x & 0x80:
                return result

    def name(self):
        n = self.leb()
        s = self.b[self.i:self.i + n].decode("utf-8")
        self.i += n
        return s

    def limits(self):
        flag = self.byte()
        self.leb()  # min
        if flag & 1:
            self.leb()  # max


def parse(data):
    if data[:4] != b"\0asm":
        raise ValueError("not a wasm module")
    r = Reader(data)
    r.i = 8
    imports, exports = [], []
    while r.i < len(data):
        section = r.byte()
        size = r.leb()
        end = r.i + size
        if section == 2:  # import section
            for _ in range(r.leb()):
                module, field = r.name(), r.name()
                imports.append(f"{module}.{field}")
                kind = r.byte()
                if kind == 0:  # function: type index
                    r.leb()
                elif kind == 1:  # table: reference type, limits
                    r.byte()
                    r.limits()
                elif kind == 2:  # memory: limits
                    r.limits()
                elif kind == 3:  # global: value type, mutability
                    r.byte()
                    r.byte()
                elif kind == 4:  # tag: attribute, type index
                    r.byte()
                    r.leb()
                else:
                    raise ValueError(f"unknown import kind {kind}")
        elif section == 7:  # export section
            for _ in range(r.leb()):
                exports.append(r.name())
                r.byte()
                r.leb()
        r.i = end
    return imports, exports


def main():
    try:
        with open(sys.argv[1], "rb") as f:
            imports, exports = parse(f.read())
    except (OSError, ValueError, IndexError, UnicodeDecodeError) as err:
        print(f"cannot parse {sys.argv[1:]}: {err}")
        sys.exit(3)
    print(f"imports={len(imports)} exports={exports}")
    for item in imports:
        print(f"  IMPORT {item}")
    sys.exit(1 if imports else (0 if "run" in exports else 2))


main()
