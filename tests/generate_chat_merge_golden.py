"""Synthetic-only oracle for export_one retention plus shard-safe merge semantics.

No imports from production Python, databases, network, chat files or key stores.
Run with --check to verify the committed fixture without writing files.
"""
import argparse
import copy
import json
from pathlib import Path


def reference(existing, incoming):
    if existing["username"] != incoming["username"]:
        return {"error": {"UsernameMismatch": {
            "existing": existing["username"], "incoming": incoming["username"]}}}
    report = dict(added=0, retained=len(existing["messages"]), duplicates=0, ambiguous=0)
    groups = {}
    for name, doc in (("existing", existing), ("incoming", incoming)):
        for index, message in enumerate(doc["messages"]):
            key = json.dumps(message["local_id"], ensure_ascii=False, separators=(",", ":"))
            groups.setdefault(key, []).append((message, dict(input=name, index=index)))
    conflicts = []
    for key in sorted(groups):
        group = groups[key]
        if len(group) > 1 and any(m.get("source") is None for m, _ in group):
            conflicts.append(dict(local_id=group[0][0]["local_id"], locations=[loc for _, loc in group]))
    if conflicts:
        report["ambiguous"] = len(conflicts)
        return {"error": {"Ambiguous": dict(report=report, conflicts=conflicts)}}
    def identity(message):
        return message.get("source"), json.dumps(message["local_id"], ensure_ascii=False)
    messages = copy.deepcopy(existing["messages"])
    seen = {identity(m) for m in messages}
    for message in incoming["messages"]:
        key = identity(message)
        if key in seen:
            report["duplicates"] += 1
        else:
            seen.add(key)
            messages.append(copy.deepcopy(message))
            report["added"] += 1
    messages.sort(key=lambda m: m.get("timestamp") or 0)
    document = copy.deepcopy(incoming)
    document.update(copy.deepcopy(existing))
    document["messages"] = messages
    return {"ok": dict(document=document, report=report)}


def build():
    def doc(messages, **extra):
        return dict(username="synthetic-user", messages=messages, **extra)
    def msg(local_id, timestamp=10, **extra):
        return dict(local_id=local_id, timestamp=timestamp, **extra)
    pairs = [
        ("boundary-null-transcription-and-shards",
         doc([msg(1, source="a", transcription="old synthetic voice", custom={"keep": True}),
              msg(2, None, source="a"), msg(3, source="a")], custom_root={"old": 1}),
         doc([msg(1, source="a", transcription="replacement"), msg(1, source="b"),
              msg(4, source="a"), msg(4, source="a"), msg(5, -1, source="b"),
              {"local_id": 6, "source": "a"}], custom_root={"new": 2}, fresh=True)),
        ("legacy-nonoverlap", doc([msg(1)]), doc([msg(2, source="a")])),
        ("legacy-cross-shard-ambiguity", doc([msg(1, transcription="keep")]),
         doc([msg(1, source="a"), msg(1, source="b")])),
        ("null-source-collision", doc([msg(1, source=None)]), doc([msg(1, source="a")])),
        ("incoming-missing-source", doc([msg(1, source="a")]), doc([msg(1)])),
        ("legacy-both-missing", doc([msg(1)]), doc([msg(1)])),
        ("incoming-unknown-collision", doc([]), doc([msg(1), msg(1)])),
        ("existing-unknown-collision", doc([msg(1), msg(1)]), doc([])),
        ("preserve-existing-known-duplicates", doc([msg(1, source="a", custom=1),
            msg(1, source="a", transcription="retained")]), doc([msg(1, source="a")])),
        ("empty", doc([]), doc([])),
        ("username-mismatch", doc([]), dict(username="other-synthetic", messages=[])),
    ]
    return {"schema": 1, "synthetic_only": True, "cases": [
        dict(name=name, existing=old, incoming=new, expected=reference(old, new))
        for name, old, new in pairs]}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    path = Path(__file__).parent / "fixtures" / "chat-merge-golden.json"
    result = build()
    if args.check:
        assert json.loads(path.read_text(encoding="utf-8")) == result, "golden differs"
        print(f"PASS: {len(result['cases'])} synthetic golden cases")
    else:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        print(f"Generated {len(result['cases'])} synthetic golden cases: {path}")
