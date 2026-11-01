import json,os
r=json.load(open(os.path.join(os.path.dirname(os.path.abspath(__file__)),"results.json")))
m=lambda b:"caught" if b else "missed"
print("| id | escape | forbid | clippy | deny | all |\n|---|---|---|---|---|---|")
for k,v in r.items():
    print(f"| {k.split('-')[0]} | {v['title']} | {m(v['forbid'])} | {m(v['clippy'])} | {m(v['deny'])} | **{m(v['all'])}** |")
bad=[k for k,v in r.items() if not v['compiles_unenforced'] or not v['all_union_agrees']]
print("\nsanity: cases that do not compile with lints capped, or where pipeline != union of layers:",bad)
