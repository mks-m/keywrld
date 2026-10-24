#!/usr/bin/env python3
"""Record keywrld on an empty Hyprland workspace with naturally paced inputs."""

import argparse
from contextlib import contextmanager
from datetime import datetime
import json
import math
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
POINTS = {
    "clear": "the clear (×) icon in the search field",
    "result": "the first search result row",
    "shift": "the left Shift key",
    "ctrl": "the left Ctrl key",
    "alt": "the left Alt key",
    "enter": "the main Enter key",
    "b": "the B key",
    "mouse_left": "the left button in the mouse diagram",
    "mouse_scroll": "the down arrow on the mouse diagram's scroll wheel",
}
MODIFIER_KEYS = {"logo": "Super_L", "shift": "Shift_L", "ctrl": "Control_L", "alt": "Alt_L"}
SCENES = (
    "Overview of all shortcuts",
    "Fuzzy search: trm → Terminal; hover the result",
    "Fuzzy search: brws; add shift to narrow by modifier",
    "Hold Super; hover Enter and mouse controls",
    "Hold Super + Shift; hover B",
    "Hold Super + Shift + Alt; hover B",
    "Click Ctrl, then Alt to latch multiple modifiers; hover a result",
    "Clear latched modifiers; scroll the full shortcut list",
    "Close with Escape",
)


def run(*args):
    return subprocess.run([str(arg) for arg in args], check=True, text=True,
                          stdout=subprocess.PIPE).stdout.strip()


def hypr_json(command):
    return json.loads(run("hyprctl", "-j", command))


def dispatch(expression):
    result = run("hyprctl", "dispatch", expression)
    if result != "ok":
        raise RuntimeError(result)


def geometry(monitor):
    return {key: monitor[key] for key in ("width", "height", "scale", "transform")}


def logical_size(monitor):
    width, height = monitor["width"], monitor["height"]
    if monitor["transform"] % 2:
        width, height = height, width
    return width / monitor["scale"], height / monitor["scale"]


class Demo:
    def __init__(self, args, monitor):
        self.args = args
        self.monitor = monitor
        self.app = None
        self.recorder = None
        self.keyboard = None
        self.points = {}
        self.original_workspace = hypr_json("activeworkspace")["id"]
        self.original_cursor = hypr_json("cursorpos")
        self.switched = False

    def pause(self, seconds):
        time.sleep(seconds * self.args.pace)

    def open(self):
        used = {workspace["id"] for workspace in hypr_json("workspaces")}
        workspace = max({0, *[number for number in used if number > 0]}) + 1
        dispatch(f"hl.dsp.focus({{workspace={workspace}}})")
        self.switched = True
        self.app = subprocess.Popen([str(self.args.binary), "--layout", "tkl"], cwd=ROOT)
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            if self.app.poll() is not None:
                raise RuntimeError("keywrld exited before its overlay appeared")
            if self.overlay_open():
                self.pause(0.7)
                return
            time.sleep(0.1)
        raise RuntimeError("Timed out waiting for the keywrld overlay")

    def overlay_open(self):
        layers = hypr_json("layers").get(self.monitor["name"], {})
        return any(layer.get("namespace") == "keywrld"
                   for level in layers.get("levels", {}).values() for layer in level)

    def calibrate(self):
        width, height = logical_size(self.monitor)
        for name, description in POINTS.items():
            run("notify-send", "-t", "6500", "keywrld demo setup",
                f"Point at {description}. Do not click. Capturing in 7 seconds.")
            time.sleep(7)
            cursor = hypr_json("cursorpos")
            x, y = cursor["x"] - self.monitor["x"], cursor["y"] - self.monitor["y"]
            if not (0 <= x < width and 0 <= y < height):
                raise ValueError(f"Pointer left the demo monitor while capturing {name}")
            self.points[name] = [x, y]
        self.args.points.parent.mkdir(parents=True, exist_ok=True)
        self.args.points.write_text(json.dumps({"monitor": self.monitor["name"],
            "geometry": geometry(self.monitor), "points": self.points}, indent=2) + "\n")
        print(f"Saved pointer calibration: {self.args.points}", flush=True)

    def load_points(self):
        profile = json.loads(self.args.points.read_text())
        if profile["monitor"] != self.monitor["name"] or profile["geometry"] != geometry(self.monitor):
            raise ValueError("Monitor or scaling changed; run --calibrate again")
        width, height = logical_size(self.monitor)
        for name in POINTS:
            x, y = profile["points"][name]
            if not (math.isfinite(x) and math.isfinite(y) and 0 <= x < width and 0 <= y < height):
                raise ValueError(f"Invalid pointer position: {name}")
        self.points = profile["points"]

    def move(self, name):
        x, y = self.points[name]
        cursor = hypr_json("cursorpos")
        start_x, start_y = cursor["x"], cursor["y"]
        target_x, target_y = x + self.monitor["x"], y + self.monitor["y"]
        # Cursor warping alone does not deliver reliable hover events to layer surfaces.
        # Use real virtual-pointer motion through wlrctl for every small movement.
        steps = 15
        for step in range(1, steps + 1):
            fraction = step / steps
            eased = fraction * fraction * (3 - 2 * fraction)
            next_x = round(start_x + (target_x - start_x) * eased)
            next_y = round(start_y + (target_y - start_y) * eased)
            current = hypr_json("cursorpos")
            run("wlrctl", "pointer", "move", next_x - current["x"], next_y - current["y"])
            self.pause(0.02)

    def click(self, name):
        self.move(name)
        run("wlrctl", "pointer", "click", "left")
        self.pause(0.5)

    def hover(self, name, seconds=2):
        self.move(name)
        self.pause(seconds)

    def type(self, text):
        run("wtype", "-d", round(160 * self.args.pace), "--", text)
        self.pause(1.5)

    @contextmanager
    def held(self, modifiers, seconds):
        # One wtype process owns each chord and releases it, even if the scene fails.
        args = ["wtype"]
        for modifier in modifiers:
            args.extend(["-P", MODIFIER_KEYS[modifier], "-M", modifier])
        args.extend(["-s", str(math.ceil(seconds * self.args.pace * 1000))])
        for modifier in reversed(modifiers):
            args.extend(["-p", MODIFIER_KEYS[modifier], "-m", modifier])
        self.keyboard = subprocess.Popen(args)
        try:
            self.pause(0.35)
            yield
        finally:
            self.keyboard.wait()
            self.keyboard = None
            self.pause(0.6)

    def record(self):
        output = self.args.output
        output.parent.mkdir(parents=True, exist_ok=True)
        # Keep diagnostics beside the recording if capture/encoding fails.
        with output.with_suffix(".log").open("w") as log:
            self.recorder = subprocess.Popen([
                "gpu-screen-recorder", "-w", self.monitor["name"], "-s", "1920x1080",
                "-f", "30", "-fm", "cfr", "-k", "h264", "-cursor", "yes",
                "-fallback-cpu-encoding", "yes", "-o", str(output),
            ], stdout=log, stderr=log)
            deadline = time.monotonic() + 15
            while not output.exists() or output.stat().st_size == 0:
                if self.recorder.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError(f"Recording did not start; see {output.with_suffix('.log')}")
                time.sleep(0.1)
            self.pause(1)
            self.sequence()
            self.stop_recorder()
        print(f"Saved recording: {output}", flush=True)

    def scene(self, index):
        if self.recorder.poll() is not None:
            raise RuntimeError("Recorder exited during the demo; check its .log file")
        if not self.overlay_open():
            raise RuntimeError("Overlay closed during the demo; stopping input")
        print(SCENES[index], flush=True)

    def sequence(self):
        self.scene(0)
        self.pause(2)
        self.scene(1)
        self.type("trm")
        self.hover("result", 2.5)
        self.click("clear")
        self.scene(2)
        self.type("brws")
        self.type(" shift")
        self.hover("result")
        self.click("clear")
        self.scene(3)
        with self.held(["logo"], 11):
            self.hover("enter")
            self.hover("mouse_left")
            self.hover("mouse_scroll", 2.5)
        self.scene(4)
        with self.held(["logo", "shift"], 4):
            self.hover("b", 2.5)
        self.scene(5)
        with self.held(["logo", "shift", "alt"], 4):
            self.hover("b", 2.5)
        self.scene(6)
        self.click("ctrl")
        self.pause(0.8)
        self.click("alt")
        self.hover("result", 2.5)
        self.click("alt")
        self.click("ctrl")
        self.scene(7)
        self.move("result")
        for _ in range(6):
            run("wlrctl", "pointer", "scroll", "120", "0")
            self.pause(0.25)
        self.pause(1.5)
        for _ in range(6):
            run("wlrctl", "pointer", "scroll", "-120", "0")
            self.pause(0.25)
        self.pause(1)
        self.scene(8)
        run("wtype", "-k", "Escape")
        self.pause(1)

    def stop_recorder(self):
        recorder, self.recorder = self.recorder, None
        if recorder is None:
            return
        if recorder.poll() is None:
            recorder.send_signal(signal.SIGINT)
        try:
            code = recorder.wait(timeout=20)
        except subprocess.TimeoutExpired:
            recorder.kill()
            recorder.wait()
            raise RuntimeError("Recorder failed to finalize the MP4")
        if code not in (0, 130, -signal.SIGINT):
            raise RuntimeError(f"Recorder exited with status {code}; check its .log file")

    def close(self):
        try:
            if self.keyboard is not None:
                self.keyboard.wait()
            self.stop_recorder()
        finally:
            try:
                if self.app is not None and self.app.poll() is None:
                    self.app.terminate()
                    try:
                        self.app.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        self.app.kill()
                        self.app.wait()
            finally:
                if self.switched:
                    dispatch(f"hl.dsp.focus({{monitor={json.dumps(self.monitor['name'])}}})")
                    dispatch(f"hl.dsp.focus({{workspace={self.original_workspace}}})")
                    cursor = self.original_cursor
                    dispatch(f"hl.dsp.cursor.move({{x={cursor['x']},y={cursor['y']}}})")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--calibrate", action="store_true", help="Save pointer positions; do not record")
    parser.add_argument("--points", type=Path, default=ROOT / "target/demo-points.json")
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/keywrld")
    parser.add_argument("--output", type=Path, default=ROOT / "target" / f"keywrld-demo-{datetime.now():%Y%m%d-%H%M%S}.mp4")
    parser.add_argument("--pace", type=float, default=1, help="Pause/typing multiplier (default: 1)")
    parser.add_argument("--dry-run", action="store_true", help="Print the sequence without accessing the desktop")
    args = parser.parse_args()
    if not math.isfinite(args.pace) or args.pace < 0.5:
        parser.error("--pace must be at least 0.5 (tooltips need time to appear)")
    if args.dry_run:
        for index, scene in enumerate(SCENES, 1):
            print(f"{index}. {scene}")
        return
    commands = ("hyprctl", "notify-send") if args.calibrate else ("hyprctl", "wtype", "wlrctl", "gpu-screen-recorder")
    missing = [command for command in commands if not shutil.which(command)]
    if missing:
        parser.error(f"Missing commands: {', '.join(missing)}")
    args.binary = args.binary.resolve()
    if not args.binary.is_file():
        parser.error("Build keywrld first: cargo build --release")
    if not args.calibrate and not args.points.is_file():
        parser.error("Run scripts/record-demo.py --calibrate first")
    if not args.calibrate and (args.output.suffix != ".mp4" or args.output.exists()):
        parser.error("--output must be a new .mp4 file")
    monitor = next(m for m in hypr_json("monitors") if m["focused"])
    width, height = logical_size(monitor)
    if width <= height:
        parser.error("Focus a landscape monitor before running the demo")
    if any(layer.get("namespace") == "keywrld" for surface in hypr_json("layers").values()
           for level in surface.get("levels", {}).values() for layer in level):
        parser.error("Close the existing keywrld overlay first")
    demo = Demo(args, monitor)
    if args.calibrate:
        print("Setup: move the pointer to each requested control without clicking.\n"
              "Each notification gives you 7 seconds; recording starts only on a later run.", flush=True)
    else:
        demo.load_points()
        print("Recording starts in 5 seconds. Leave the keyboard and mouse untouched.\n"
              "The original workspace is restored when finished.", flush=True)
    time.sleep(5)
    try:
        demo.open()
        if args.calibrate:
            demo.calibrate()
        else:
            demo.record()
    finally:
        demo.close()


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    try:
        main()
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.SubprocessError) as error:
        sys.exit(f"Demo failed: {error}")
    except KeyboardInterrupt:
        sys.exit("Demo interrupted; cleanup finished")
