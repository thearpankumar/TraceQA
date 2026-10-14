"""Score `traceqa extract` against the Buggy Shop.

    uv run bench/run.py [--browsers chromium,firefox,webkit] [--repeats 3] [--json scores.json]

Starts the shop, runs extraction in every browser at every viewport in
fixtures/buggy-shop/manifest.yaml, and compares what was found with what the
manifest says is there. Exits 1 when anything is missing, extra or wrong, or when
ids differ between runs.

Scores:
  found       expected components that were found
  extra       components reported that the manifest does not list
  wrong       found components whose instance count, confidence, visibility,
              disabled count, frame or shadow flag differs from the manifest
  id stable   the same ids on every repeat, in every browser, at every viewport
  time        median seconds per page
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import statistics
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

import yaml

ROOT = Path(__file__).resolve().parent.parent
SHOP = ROOT / "fixtures" / "buggy-shop"


def find_binary() -> Path:
    name = "traceqa.exe" if os.name == "nt" else "traceqa"
    candidates = [os.environ.get("TRACEQA_BIN"), ROOT / "target" / "release" / name, ROOT / "target" / "debug" / name]
    for candidate in candidates:
        if candidate and Path(candidate).is_file():
            return Path(candidate)
    sys.exit("traceqa binary not found. Run `cargo build` or set TRACEQA_BIN.")


def start_shop() -> str:
    spec = importlib.util.spec_from_file_location("buggy_shop_serve", SHOP / "serve.py")
    shop = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(shop)
    port, _servers = shop.start_on_free_ports()
    return f"http://127.0.0.1:{port}/"


def matches(expected: dict[str, Any], found: dict[str, Any]) -> bool:
    if found["role"] != expected["role"]:
        return False
    if expected.get("template"):
        return found["nameIsTemplate"]
    return not found["nameIsTemplate"] and found["name"] == expected["name"]


def problems(expected: dict[str, Any], found: dict[str, Any], viewport: str) -> list[str]:
    """Ways a found component differs from what the manifest says about it."""
    out = []
    instances = found["instances"]

    def check(what: str, want: Any, got: Any) -> None:
        if want != got:
            out.append(f"{what}: expected {want}, got {got}")

    check("instances", expected.get("instances", 1), len(instances))
    check("confidence", expected.get("confidence", "high"), found["confidence"])
    check("visible", viewport not in expected.get("hidden", []), any(i["state"]["visible"] for i in instances))
    check("disabled", expected.get("disabled", 0), sum(not i["state"]["enabled"] for i in instances))
    check("shadow", bool(expected.get("shadow")), all(i["inShadowDom"] for i in instances))
    frame = expected.get("frame")
    in_frame = found["path"].split(" > ")[0] if found["path"].startswith("frame ") else None
    check("frame", f'frame "{frame}"' if frame else None, in_frame)
    return out


def score_target(manifest: dict[str, Any], components_file: dict[str, Any], viewport: str) -> dict[str, Any]:
    remaining = list(components_file["components"])
    missing, wrong = [], []
    for expected in manifest["components"]:
        label = f"{expected['role']} {'(template)' if expected.get('template') else repr(expected['name'])}"
        found = next((component for component in remaining if matches(expected, component)), None)
        if found is None:
            missing.append(label)
            continue
        remaining.remove(found)
        for problem in problems(expected, found, viewport):
            wrong.append(f"{label}: {problem}")

    extra = [f"{c['role']} {c['name']!r}" for c in remaining]
    for absent in manifest.get("absent", []):
        if any(matches(absent, component) for component in components_file["components"]):
            wrong.append(f"{absent['role']} {absent['name']!r}: only exists after a click and must not be reported")
    warning_codes = sorted({warning["code"] for warning in components_file["warnings"]})
    if warning_codes != sorted(manifest.get("warnings", [])):
        wrong.append(f"warnings: expected {sorted(manifest.get('warnings', []))}, got {warning_codes}")
    return {
        "expected": len(manifest["components"]),
        "found": len(manifest["components"]) - len(missing),
        "missing": missing,
        "extra": extra,
        "wrong": wrong,
        "ids": [component["id"] for component in components_file["components"]],
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--browsers", default="chromium,firefox,webkit")
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--json", type=Path, help="also write the scores to this file")
    args = parser.parse_args()

    manifest = yaml.safe_load((SHOP / "manifest.yaml").read_text(encoding="utf-8"))
    viewports = {f"{size['width']}x{size['height']}": name for name, size in manifest["viewports"].items()}
    binary = find_binary()
    url = start_shop()

    rows: dict[str, dict[str, Any]] = {}
    with tempfile.TemporaryDirectory(prefix="traceqa-bench-") as out_dir:
        for _ in range(args.repeats):
            command = [str(binary), "extract", url, "--browser", args.browsers, "--viewport", ",".join(viewports), "--out", out_dir, "--crops", "--json"]  # fmt: skip
            done = subprocess.run(command, capture_output=True, check=False, text=True, encoding="utf-8")
            if not done.stdout.strip():
                sys.exit(f"traceqa produced no manifest:\n{done.stderr}")
            run = json.loads(done.stdout)
            for target in run["targets"]:
                row = rows.setdefault(target["dir"], {"scores": [], "seconds": [], "errors": []})
                if target["status"] != "completed":
                    row["errors"].append(target.get("error") or target["status"])
                    continue
                path = Path(out_dir) / run["runId"] / target["dir"] / "components.json"
                components_file = json.loads(path.read_text(encoding="utf-8"))
                viewport = viewports[f"{target['viewport']['width']}x{target['viewport']['height']}"]
                row["scores"].append(score_target(manifest, components_file, viewport))
                row["seconds"].append(target["durationMs"] / 1000)

    all_ids = {tuple(score["ids"]) for row in rows.values() for score in row["scores"]}
    ids_stable = len(all_ids) == 1
    failed = not ids_stable

    print(f"\n{'TARGET':<22} {'FOUND':>7} {'EXTRA':>6} {'WRONG':>6} {'TIME':>7}")
    report = {"idsStable": ids_stable, "repeats": args.repeats, "targets": {}}
    for name, row in rows.items():
        if row["errors"] or not row["scores"]:
            failed = True
            print(f"{name:<22} FAILED: {'; '.join(sorted(set(row['errors']))) or 'no result'}")
            report["targets"][name] = {"errors": row["errors"]}
            continue
        # Report the worst repeat, so one flaky run cannot hide.
        worst = min(row["scores"], key=lambda s: (s["found"], -len(s["extra"]), -len(s["wrong"])))
        seconds = statistics.median(row["seconds"])
        print(
            f"{name:<22} {worst['found']:>3}/{worst['expected']:<3} {len(worst['extra']):>6} {len(worst['wrong']):>6} {seconds:>6.1f}s"
        )
        for kind in ("missing", "extra", "wrong"):
            for item in worst[kind]:
                print(f"    {kind}: {item}")
        failed = failed or worst["found"] < worst["expected"] or bool(worst["extra"]) or bool(worst["wrong"])
        report["targets"][name] = {key: worst[key] for key in ("expected", "found", "missing", "extra", "wrong")} | {
            "medianSeconds": seconds
        }

    runs = sum(len(row["scores"]) for row in rows.values())
    print(
        f"\nIds stable across {runs} runs ({args.repeats} repeats, every browser and viewport): {'yes' if ids_stable else 'NO'}"
    )
    if not ids_stable:
        print(f"    {len(all_ids)} different id lists were produced")
    if args.json:
        args.json.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print("\nFAIL" if failed else "\nPASS")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
