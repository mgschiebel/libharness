"""Print the import section of a wasm module as module.name lines; exit 1 if any, 2 if no `run` export."""
import sys
b = open(sys.argv[1], 'rb').read()
assert b[:4] == b'\0asm'
def leb(i):
    r = s = 0
    while True:
        x = b[i]; i += 1; r |= (x & 0x7f) << s; s += 7
        if not x & 0x80: return r, i
def name(i):
    n, i = leb(i); return b[i:i+n].decode(), i+n
i, imports, exports = 8, [], []
while i < len(b):
    sid = b[i]; size, j = leb(i+1); end = j+size
    if sid == 2:
        n, j = leb(j)
        for _ in range(n):
            m, j = name(j); f, j = name(j); imports.append(f"{m}.{f}")
            k = b[j]; j += 1
            if k == 0: _, j = leb(j)
            elif k == 1: j += 1; fl = b[j]; j += 1; _, j = leb(j); (fl & 1) and leb(j)[1] and None
            else: break  # memory/global imports: report and stop parsing
    if sid == 7:
        n, j = leb(j)
        for _ in range(n):
            f, j = name(j); j += 1; _, j = leb(j); exports.append(f)
    i = end
print(f"imports={len(imports)} exports={exports}")
for x in imports: print("  IMPORT", x)
sys.exit(1 if imports else (0 if 'run' in exports else 2))
