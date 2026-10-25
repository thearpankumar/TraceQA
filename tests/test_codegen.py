"""`traceqa codegen`: the page objects it writes find what the extraction found, in every browser.

The module fixture reads the Buggy Shop in Chromium, Firefox and WebKit (and Chromium at phone
size), writes page objects in both languages, and runs them for real: the Python one with
Playwright's sync API, the TypeScript one with `npx playwright test` in tests/codegen_ts. Each
property must find exactly as many elements as that browser's components.json recorded. If the
engine's idea of a role or a name and Playwright's ever differ, these fail; do not loosen them.
"""

from __future__ import annotations

import ast
import copy
import importlib.util
import json
import os
import re
import shutil
import subprocess
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest
import tomllib
from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parents[1]
TS_DIR = ROOT / "tests" / "codegen_ts"
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)
BROWSERS = ["chromium", "firefox", "webkit"]
DESKTOP = (1280, 800)
PHONE = (390, 844)

pytestmark = pytest.mark.skipif(not BINARY.is_file(), reason="build the engine first: cargo build")


def run_cli(*args: str | Path, timeout: int = 300) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(BINARY), *map(str, args)],
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=timeout,
    )


def extract(url: str, out: Path, browsers: list[str], viewport: tuple[int, int]) -> Path:
    result = run_cli(
        "extract",
        url,
        "--browser",
        ",".join(browsers),
        "--viewport",
        f"{viewport[0]}x{viewport[1]}",
        "--out",
        out,
    )
    assert result.returncode == 0, result.stderr
    (folder,) = out.iterdir()
    return folder


def codegen(run: Path, lang: str, out: Path, *extra: str) -> None:
    result = run_cli("codegen", run, "--lang", lang, "--out", out, *extra)
    assert result.returncode == 0, result.stderr + result.stdout


@pytest.fixture(scope="module")
def desktop_run(shop_url: str, tmp_path_factory: pytest.TempPathFactory) -> Path:
    return extract(shop_url, tmp_path_factory.mktemp("desktop"), BROWSERS, DESKTOP)


@pytest.fixture(scope="module")
def phone_run(shop_url: str, tmp_path_factory: pytest.TempPathFactory) -> Path:
    return extract(shop_url, tmp_path_factory.mktemp("phone"), ["chromium"], PHONE)


def components_of(run: Path, target: str) -> dict[str, Any]:
    return {
        c["id"]: c for c in json.loads((run / target / "components.json").read_text(encoding="utf-8"))["components"]
    }


def expected_counts(run: Path, target: str, locators: dict[str, Any]) -> dict[str, int]:
    """What each generated property must find in `target`, from that browser's own components.json."""
    found = components_of(run, target)
    counts: dict[str, int] = {}
    for component_id, info in locators["components"].items():
        component = found.get(component_id)
        if component is None:
            counts[info["property"]] = 0
            continue
        instances = component["instances"]
        # CSS ignores visibility, and a hidden-only component is found with includeHidden.
        hidden_only = not any(i["state"]["visible"] for i in instances)
        if info["forced"] or hidden_only:
            counts[info["property"]] = len(instances)
        else:
            counts[info["property"]] = sum(1 for i in instances if i["state"]["visible"])
    return counts


def scroll_through(page: Any) -> None:
    """Load what the shop loads while a person scrolls (the reviews), as the extraction did."""
    height = -1
    for _ in range(12):
        page.evaluate("window.scrollTo(0, document.body.scrollHeight)")
        page.wait_for_timeout(300)
        now = page.evaluate("document.body.scrollHeight")
        if now == height:
            break
        height = now
    page.evaluate("window.scrollTo(0, 0)")


def load_module(path: Path) -> ModuleType:
    spec = importlib.util.spec_from_file_location(f"generated_{path.stem}", path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.fixture(scope="module")
def py_pages(desktop_run: Path, tmp_path_factory: pytest.TempPathFactory) -> Path:
    out = tmp_path_factory.mktemp("py-pages")
    codegen(desktop_run, "py", out)
    return out


@pytest.fixture(scope="module")
def ts_pages(desktop_run: Path, tmp_path_factory: pytest.TempPathFactory) -> Path:
    out = tmp_path_factory.mktemp("ts-pages")
    codegen(desktop_run, "ts", out)
    return out


def test_codegen_writes_a_page_object_and_locators_json(py_pages: Path, ts_pages: Path, desktop_run: Path) -> None:
    assert sorted(p.name for p in py_pages.iterdir()) == ["buggy_shop_page.py", "locators.json"]
    assert sorted(p.name for p in ts_pages.iterdir()) == ["buggy-shop.page.ts", "locators.json"]
    locators = json.loads((py_pages / "locators.json").read_text(encoding="utf-8"))
    assert locators["language"] == "py"
    assert locators["className"] == "BuggyShopPage"
    assert locators["targets"] == [f"{b}-{DESKTOP[0]}x{DESKTOP[1]}" for b in BROWSERS]
    ids = set(locators["components"])
    assert ids <= set(components_of(desktop_run, locators["targets"][0]))
    assert len(ids) >= 30
    for info in locators["components"].values():
        assert set(info) >= {"property", "locator", "presentIn", "uniqueIn", "unique", "forced"}


def test_every_property_has_a_marker_and_locators_json_agrees(py_pages: Path, ts_pages: Path) -> None:
    for folder, comment, pattern in [
        (py_pages, "#", r"^    @property\n    def (\w+)\(self\)"),
        (ts_pages, "//", r"^  get (\w+)\(\): Locator"),
    ]:
        locators = json.loads((folder / "locators.json").read_text(encoding="utf-8"))
        source = next(p for p in folder.iterdir() if p.suffix in {".py", ".ts"}).read_text(encoding="utf-8")
        markers = re.findall(rf"^ +{comment} traceqa:id (c_[0-9a-f]{{12}})$", source, re.MULTILINE)
        assert sorted(markers) == sorted(locators["components"]), folder
        properties = re.findall(pattern, source, re.MULTILINE)
        assert sorted(properties) == sorted(info["property"] for info in locators["components"].values())
        assert len(set(properties)) == len(properties)
        # A marker sits directly before its property, so a tool that rewrites by marker finds it.
        for component_id, info in locators["components"].items():
            index = source.index(f"traceqa:id {component_id}")
            assert info["property"] in source[index : index + 1200]


def test_nth_is_never_written_and_forced_properties_say_so(py_pages: Path, ts_pages: Path) -> None:
    for folder in (py_pages, ts_pages):
        source = next(p for p in folder.iterdir() if p.suffix in {".py", ".ts"}).read_text(encoding="utf-8")
        assert ".nth(" not in source
        locators = json.loads((folder / "locators.json").read_text(encoding="utf-8"))
        forced = [i for i, info in locators["components"].items() if info["forced"]]
        assert forced, "the shop has an unnamed icon button, which can only be found by CSS"
        assert source.count("FORCED") == len(forced)
        for component_id in forced:
            index = source.index(f"traceqa:id {component_id}")
            assert "FORCED" in source[index : source.index("\n\n", index)]


def test_the_python_page_object_finds_what_each_browser_extracted(
    py_pages: Path, desktop_run: Path, shop_url: str
) -> None:
    locators = json.loads((py_pages / "locators.json").read_text(encoding="utf-8"))
    module = load_module(py_pages / "buggy_shop_page.py")
    failures: list[str] = []
    with sync_playwright() as playwright:
        for browser_name in BROWSERS:
            target = f"{browser_name}-{DESKTOP[0]}x{DESKTOP[1]}"
            wanted = expected_counts(desktop_run, target, locators)
            browser = getattr(playwright, browser_name).launch()
            try:
                page = browser.new_page(viewport={"width": DESKTOP[0], "height": DESKTOP[1]})
                page.goto(shop_url)
                scroll_through(page)
                shop = module.BuggyShopPage(page)
                for component_id, info in locators["components"].items():
                    got = getattr(shop, info["property"]).count()
                    if got != wanted[info["property"]]:
                        failures.append(
                            f"{browser_name} {info['property']} ({component_id}): {got}, wanted {wanted[info['property']]}"
                        )
            finally:
                browser.close()
    assert not failures, "\n".join(failures)


def test_the_python_page_object_at_phone_size(phone_run: Path, shop_url: str, tmp_path: Path) -> None:
    codegen(phone_run, "py", tmp_path)
    locators = json.loads((tmp_path / "locators.json").read_text(encoding="utf-8"))
    module = load_module(tmp_path / "buggy_shop_page.py")
    target = f"chromium-{PHONE[0]}x{PHONE[1]}"
    wanted = expected_counts(phone_run, target, locators)
    failures: list[str] = []
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch()
        try:
            page = browser.new_page(viewport={"width": PHONE[0], "height": PHONE[1]})
            page.goto(shop_url)
            scroll_through(page)
            shop = module.BuggyShopPage(page)
            for info in locators["components"].values():
                got = getattr(shop, info["property"]).count()
                if got != wanted[info["property"]]:
                    failures.append(f"{info['property']}: {got}, wanted {wanted[info['property']]}")
        finally:
            browser.close()
    assert not failures, "\n".join(failures)


def ensure_node_modules() -> list[str]:
    node = shutil.which("node")
    assert node, "node is not installed: the TypeScript page object cannot be tested"
    if not (TS_DIR / "node_modules" / "@playwright" / "test").is_dir():
        npm = shutil.which("npm")
        assert npm, "npm is not installed"
        subprocess.run([npm, "ci"], cwd=TS_DIR, check=True, capture_output=True, timeout=600)
    return [node, str(TS_DIR / "node_modules" / "@playwright" / "test" / "cli.js")]


def test_the_typescript_page_object_runs_in_every_browser(
    ts_pages: Path, desktop_run: Path, shop_url: str, tmp_path: Path
) -> None:
    locators = json.loads((ts_pages / "locators.json").read_text(encoding="utf-8"))
    counts = {
        browser: expected_counts(desktop_run, f"{browser}-{DESKTOP[0]}x{DESKTOP[1]}", locators) for browser in BROWSERS
    }
    expected = tmp_path / "expected.json"
    expected.write_text(json.dumps({"className": locators["className"], "counts": counts}), encoding="utf-8")
    command = ensure_node_modules()
    result = subprocess.run(
        [*command, "test"],
        cwd=TS_DIR,
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=600,
        env={
            **os.environ,
            "TRACEQA_SHOP_URL": shop_url,
            "TRACEQA_PAGE_OBJECT": str(ts_pages / "buggy-shop.page.ts"),
            "TRACEQA_EXPECTED": str(expected),
            "CI": "1",
        },
    )
    assert result.returncode == 0, result.stdout + result.stderr
    assert "3 passed" in result.stdout, result.stdout


def test_the_typescript_tests_use_the_playwright_version_of_the_python_tests() -> None:
    """The cached browsers are chosen by the Playwright version; two versions would need two sets."""
    lock = tomllib.loads((ROOT / "uv.lock").read_text(encoding="utf-8"))
    (python_version,) = {p["version"] for p in lock["package"] if p["name"] == "playwright"}
    package = json.loads((TS_DIR / "package.json").read_text(encoding="utf-8"))
    assert package["devDependencies"]["@playwright/test"] == python_version
    npm_lock = json.loads((TS_DIR / "package-lock.json").read_text(encoding="utf-8"))
    assert npm_lock["packages"]["node_modules/@playwright/test"]["version"] == python_version
    assert npm_lock["packages"]["node_modules/playwright"]["version"] == python_version


def test_codegen_refuses_a_run_from_before_locators(tmp_path: Path) -> None:
    run = tmp_path / "r_old"
    write_run(run, strip_locators=True)
    result = run_cli("codegen", run, "--lang", "py", "--out", tmp_path / "out")
    assert result.returncode == 2
    assert "Extract the page again" in result.stderr


def test_codegen_checks_its_arguments(tmp_path: Path) -> None:
    run = tmp_path / "r_ok"
    write_run(run)
    for args, says in [
        (["--class-name", "a b"], "is not a name"),
        (["--target", "safari"], "no completed target matches"),
    ]:
        result = run_cli("codegen", run, "--lang", "ts", "--out", tmp_path / "out", *args)
        assert result.returncode == 2, args
        assert says in result.stderr
    missing = run_cli("codegen", tmp_path / "nowhere", "--lang", "ts")
    assert missing.returncode == 2
    assert "is not a run folder" in missing.stderr


def test_a_target_folder_name_cannot_leave_the_run_folder(tmp_path: Path) -> None:
    run = tmp_path / "r_evil"
    write_run(run)
    manifest = json.loads((run / "run.json").read_text(encoding="utf-8"))
    manifest["targets"][0]["dir"] = "../elsewhere"
    (run / "run.json").write_text(json.dumps(manifest), encoding="utf-8")
    result = run_cli("codegen", run, "--lang", "py", "--out", tmp_path / "out")
    assert result.returncode == 2
    assert "plain folder name" in result.stderr


# Page text that tries to end a string, a comment or a line.
HOSTILE = {
    "quote": '"); process.exit(1); //',
    "newline": "line1\nline2\r\n\u2028end\u2029",
    "backslash": 'C:\\path\\"quoted"\\',
    "rtl": "\u05e9\u05dc\u05d5\u05dd \u202e evil",
    "hindi": "\u0915\u093e\u0930\u094d\u091f \u0926\u0947\u0916\u0947\u0902",
    "closers": "*/ x /* ''' \"\"\" \n# ${process.exit(1)} `",
}
TARGET = "chromium-1280x800"


def hostile_components() -> list[dict[str, Any]]:
    """Components whose names, scopes, frame, selector and test id are all hostile."""
    base = json.loads((ROOT / "spec" / "examples" / "ComponentsFile.shop.json").read_text(encoding="utf-8"))
    template = base["components"][0]

    def make(index: int, role: str, name: str, locator: dict[str, Any]) -> dict[str, Any]:
        component = copy.deepcopy(template)
        component["id"] = f"c_{index:012x}"
        component["role"] = role
        component["name"] = name
        component["locator"] = {
            "unique": True,
            "matches": 1,
            "own": 1,
            "forced": False,
            "needsIncludeHidden": False,
            **locator,
        }
        for n, instance in enumerate(component["instances"]):
            instance["key"] = f"{component['id']}:{n}"
        return component

    return [
        make(
            1,
            "button",
            HOSTILE["quote"],
            {"strategy": "role", "role": "button", "name": HOSTILE["quote"], "exact": True},
        ),
        make(
            2,
            "link",
            HOSTILE["newline"],
            {
                "strategy": "role",
                "role": "link",
                "name": HOSTILE["newline"],
                "exact": True,
                "scope": [{"role": "dialog", "name": HOSTILE["backslash"]}, {"role": "form"}],
                "frame": HOSTILE["rtl"].lower(),
            },
        ),
        make(3, "generic", HOSTILE["hindi"], {"strategy": "text", "name": HOSTILE["hindi"], "exact": True}),
        make(
            4,
            HOSTILE["closers"],
            "a",
            {"strategy": "css", "css": 'a"]; alert(1); //\n', "forced": True},
        ),
        make(5, "button", "t", {"strategy": "testid", "testId": HOSTILE["closers"]}),
        make(
            6,
            "textbox",
            HOSTILE["rtl"],
            {"strategy": "role", "role": "textbox", "name": HOSTILE["rtl"], "exact": True, "needsIncludeHidden": True},
        ),
    ]


def write_run(folder: Path, *, hostile: bool = False, strip_locators: bool = False) -> None:
    """A minimal finished run with one target, built from the spec's examples."""
    examples = ROOT / "spec" / "examples"
    manifest = json.loads((examples / "RunManifest.completed.json").read_text(encoding="utf-8"))
    manifest["targets"][0]["dir"] = TARGET
    manifest["targets"][0]["target"] = {"browser": "chromium", "headless": True}
    file = json.loads((examples / "ComponentsFile.shop.json").read_text(encoding="utf-8"))
    if hostile:
        file["components"] = hostile_components()
    if strip_locators:
        for component in file["components"]:
            component.pop("locator", None)
    (folder / TARGET).mkdir(parents=True)
    (folder / "run.json").write_text(json.dumps(manifest), encoding="utf-8")
    (folder / TARGET / "components.json").write_text(json.dumps(file), encoding="utf-8")


def expected_chains() -> dict[str, list[list[Any]]]:
    """The calls each hostile component must make, written out by hand: ts spelling."""
    h = HOSTILE
    frame = ["frame"]
    return {
        "quote": [["getByRole", "button", {"name": h["quote"], "exact": True}]],
        "newline": [
            frame,
            ["getByRole", "dialog", {"name": h["backslash"], "exact": True}],
            ["getByRole", "form"],
            ["getByRole", "link", {"name": h["newline"], "exact": True}],
        ],
        "hindi": [["getByText", h["hindi"], {"exact": True}]],
        "css": [["locator", 'a"]; alert(1); //\n']],
        "testid": [["getByTestId", h["closers"]]],
        "rtl": [["getByRole", "textbox", {"name": h["rtl"], "exact": True, "includeHidden": True}]],
    }


def hostile_run(tmp_path: Path) -> Path:
    run = tmp_path / "r_hostile"
    write_run(run, hostile=True)
    return run


def test_hostile_page_text_cannot_leave_its_string_in_python(tmp_path: Path) -> None:
    run = hostile_run(tmp_path)
    out = tmp_path / "out"
    codegen(run, "py", out, "--include-hidden")
    path = out / "buggy_shop_page.py"
    source = path.read_text(encoding="utf-8")
    tree = ast.parse(source)  # it must at least be Python
    # Nothing but imports, functions and the one class; the class holds only functions that return.
    assert {type(node) for node in tree.body} <= {ast.Import, ast.ImportFrom, ast.FunctionDef, ast.ClassDef}
    (shop,) = [node for node in tree.body if isinstance(node, ast.ClassDef)]
    for node in shop.body:
        assert isinstance(node, ast.FunctionDef)
        statements = [s for s in node.body if not (isinstance(s, ast.Expr) and isinstance(s.value, ast.Constant))]
        assert node.name == "__init__" or [type(s) for s in statements] == [ast.Return], node.name
    # Identifiers are plain ASCII whatever the names were.
    assert all(re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", n.name) for n in shop.body if isinstance(n, ast.FunctionDef))
    assert all(ord(c) < 128 or c in HOSTILE["hindi"] + HOSTILE["rtl"] for c in source if ord(c) > 127)
    # Run it against a page that records calls: the strings arrive unchanged.
    got = record_python(path, "BuggyShopPage", HOSTILE["rtl"].lower())
    want = [[normalise(call) for call in chain if call != ["frame"]] for chain in expected_chains().values()]
    assert list(got.values()) == want


def normalise(chain: list[Any]) -> tuple[str, list[Any], dict[str, Any]]:
    """The ts spelling of one call as the python recorder reports it."""
    name, *rest = chain
    snake = re.sub(r"(?<!^)([A-Z])", lambda m: "_" + m.group(1).lower(), name)
    options = rest[-1] if rest and isinstance(rest[-1], dict) else {}
    args = rest[:-1] if options else rest
    return (
        snake,
        args,
        {re.sub(r"(?<!^)([A-Z])", lambda m: "_" + m.group(1).lower(), k): v for k, v in options.items()},
    )


def record_python(
    path: Path, class_name: str, frame_name: str
) -> dict[str, list[tuple[str, list[Any], dict[str, Any]]]]:
    class Recorder:
        def __init__(self, calls: list[Any], **own: Any) -> None:
            self.__dict__["_calls"] = calls
            self.__dict__.update(own)

        def __getattr__(self, name: str) -> Any:
            if name.startswith("__"):
                raise AttributeError(name)
            return lambda *args, **kwargs: Recorder([*self._calls, (name, list(args), kwargs)])

    main = object()
    frame = Recorder([("frame", [], {})], name=frame_name, url="about:blank")
    page = Recorder([], frames=[main, frame], main_frame=main)
    module = load_module(path)
    shop = getattr(module, class_name)(page)
    properties = [n for n, v in vars(getattr(module, class_name)).items() if isinstance(v, property)]
    out: dict[str, list[tuple[str, list[Any], dict[str, Any]]]] = {}
    for prop in properties:
        calls = getattr(shop, prop)._calls
        out[prop] = [c for c in calls if c[0] != "frame"]
    return out


def test_hostile_page_text_cannot_leave_its_string_in_typescript(tmp_path: Path) -> None:
    run = hostile_run(tmp_path)
    out = tmp_path / "out"
    codegen(run, "ts", out, "--include-hidden")
    path = out / "buggy-shop.page.ts"
    source = path.read_text(encoding="utf-8")
    assert all(ord(c) < 128 or c in HOSTILE["hindi"] + HOSTILE["rtl"] for c in source)
    node = shutil.which("node")
    assert node, "node is not installed: the TypeScript page object cannot be tested"
    result = subprocess.run(
        [node, str(TS_DIR / "record.mjs"), str(path), "BuggyShopPage", HOSTILE["rtl"].lower()],
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=60,
    )
    assert result.returncode == 0, result.stderr
    recorded = json.loads(result.stdout)
    assert list(recorded.values()) == list(expected_chains().values())
    # Also valid TypeScript syntax once the types are stripped: record.mjs imported it.
    # No property name came from page text.
    assert all(re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name) for name in recorded)


def test_hostile_text_never_starts_a_line_of_the_source(tmp_path: Path) -> None:
    """A line break in a name must not start a new line of code or comment."""
    hostile = hostile_run(tmp_path)
    for lang in ("py", "ts"):
        out = tmp_path / f"out-{lang}"
        codegen(hostile, lang, out, "--include-hidden")
        source = next(p for p in out.iterdir() if p.suffix in {".py", ".ts"}).read_text(encoding="utf-8")
        for line in source.splitlines():
            assert not line.strip().startswith(("line2", "end", "# $", "x /*", "alert(1)", "process.exit")), line
        # Every U+2028 or bidi override in a name is written as an escape, never raw.
        assert chr(0x2028) not in source
        assert chr(0x202E) not in source


def test_ci_installs_the_codegen_test_tools_before_the_python_tests() -> None:
    """G3: the TypeScript half must run in CI, so its npm packages are installed there."""
    import yaml

    ci = yaml.safe_load((ROOT / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8"))
    steps = ci["jobs"]["test"]["steps"]
    install = next(i for i, s in enumerate(steps) if s.get("working-directory") == "tests/codegen_ts")
    assert steps[install]["run"].strip() == "npm ci"
    pytest_step = next(i for i, s in enumerate(steps) if s.get("run", "").strip() == "uv run pytest")
    browsers = next(i for i, s in enumerate(steps) if "playwright install" in s.get("run", ""))
    assert browsers < install < pytest_step
    node = next(s for s in steps if str(s.get("uses", "")).startswith("actions/setup-node"))
    assert "tests/codegen_ts/package-lock.json" in node["with"]["cache-dependency-path"]
