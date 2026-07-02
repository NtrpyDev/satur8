#!/usr/bin/env python3
"""Automate the CS2 FPS BENCHMARK DUST2 Satur8 cost test.

This is intentionally a local PC1 automation harness, not app/runtime code. It
drives the existing desktop with ydotool, screenshots with Spectacle, copies or
OCRs the CS2 console VProf result, catalogs raw + parsed data, quits CS2 after
every attempt, and relaunches for the next attempt.

Default validation is strict: only runs with zero excluded frames and the
expected 129 one-second intervals are accepted. Dirty attempts are preserved in
the dataset but retried until the requested number of accepted pairs is reached.
"""

from __future__ import annotations

import argparse
import csv
import json
import os
import re
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Iterable

from PIL import Image


ROOT = Path(__file__).resolve().parents[1]
BENCH_ROOT = ROOT / "benchmarks"

KEY_ESC = 1
KEY_MINUS = 12
KEY_ENTER = 28
KEY_LEFTCTRL = 29
KEY_A = 30
KEY_C = 46


def log(message: str) -> None:
    print(message, flush=True)


@dataclass
class RunResult:
    mode: str
    saturation: float
    pair_index: int
    attempt_index: int
    sequence_index: int
    accepted: bool
    rejection_reasons: list[str]
    raw_console_text_path: str
    timestamp: str
    avg_fps: float
    p1_fps: float
    frames_total: int
    frames_excluded: int
    one_second_intervals: int
    frame_total_avg_ms: float | None
    frame_total_p99_ms: float | None
    client_rendering_avg_ms: float | None
    client_rendering_p99_ms: float | None
    present_renderdevice_avg_ms: float | None
    present_renderdevice_p99_ms: float | None


def run(cmd: list[str], *, check: bool = True, capture: bool = False, input_text: str | None = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        cmd,
        check=check,
        text=True,
        input=input_text,
        stdout=subprocess.PIPE if capture else subprocess.DEVNULL,
        stderr=subprocess.PIPE if capture else subprocess.DEVNULL,
    )


def require_tools(names: Iterable[str]) -> None:
    missing = [name for name in names if shutil.which(name) is None]
    if missing:
        raise SystemExit(f"missing required tool(s): {', '.join(missing)}")


def ydotool_key(*events: str) -> None:
    run(["ydotool", "key", *events])


def ydotool_type(text: str) -> None:
    run(["ydotool", "type", "-d", "15", text])


def ctrl_key(key_code: int) -> None:
    ydotool_key(f"{KEY_LEFTCTRL}:1", f"{key_code}:1", f"{key_code}:0", f"{KEY_LEFTCTRL}:0")


def click(x: int, y: int) -> None:
    run(["ydotool", "mousemove", "-a", str(x), str(y)], check=False)
    time.sleep(0.08)
    run(["ydotool", "click", "0xC0"], check=False)
    time.sleep(0.25)


def screenshot(path: Path) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    run(["spectacle", "-b", "-n", "-o", str(path)], check=False)
    return path


def left_monitor_image(path: Path, width: int, height: int) -> Image.Image:
    img = Image.open(path).convert("RGB")
    return img.crop((0, 0, min(width, img.width), min(height, img.height)))


def ocr_image(path: Path) -> str:
    proc = run(["tesseract", str(path), "stdout", "--psm", "6"], check=False, capture=True)
    return (proc.stdout or "") + "\n" + (proc.stderr or "")


def crop_left_to_file(src: Path, dest: Path, width: int, height: int) -> Path:
    img = left_monitor_image(src, width, height)
    dest.parent.mkdir(parents=True, exist_ok=True)
    img.save(dest)
    return dest


def find_green_go_button(img: Image.Image) -> tuple[int, int] | None:
    width, height = img.size
    xs: list[int] = []
    ys: list[int] = []
    for y in range(int(height * 0.72), height):
        for x in range(int(width * 0.45), width):
            r, g, b = img.getpixel((x, y))
            if g > 95 and g > r * 1.35 and g > b * 1.35 and (g - max(r, b)) > 45:
                xs.append(x)
                ys.append(y)
    if len(xs) < 800:
        return None
    return (round((min(xs) + max(xs)) / 2), round((min(ys) + max(ys)) / 2))


def find_ocr_word(path: Path, word_pattern: str) -> tuple[int, int] | None:
    proc = run(["tesseract", str(path), "stdout", "--psm", "6", "tsv"], check=False, capture=True)
    pattern = re.compile(word_pattern, re.I)
    for line in (proc.stdout or "").splitlines()[1:]:
        cols = line.split("\t")
        if len(cols) < 12:
            continue
        text = cols[11].strip()
        if not text or not pattern.fullmatch(text):
            continue
        try:
            conf = float(cols[10])
            left, top, width, height = map(int, cols[6:10])
        except ValueError:
            continue
        if conf >= 35:
            return (left + width // 2, top + height // 2)
    return None


def wait_for_cs2_process(timeout: int) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        proc = run(["pgrep", "-n", "cs2"], check=False, capture=True)
        if proc.stdout.strip():
            return True
        time.sleep(2)
    return False


def wait_for_cs2_exit(timeout: int) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        proc = run(["pgrep", "-n", "cs2"], check=False, capture=True)
        if not proc.stdout.strip():
            return True
        time.sleep(2)
    return False


def launch_cs2(args: argparse.Namespace, art: Path) -> None:
    if args.steam_launch in {"auto", "click"}:
        log("launch: trying Steam Play button")
        clicked = try_click_steam_play(args, art)
        if clicked and wait_for_cs2_process(args.click_launch_grace):
            log("launch: CS2 process detected after Steam Play click")
            return
        if clicked:
            log("launch: Steam Play click did not spawn CS2")
        if args.steam_launch == "click":
            raise RuntimeError("could not find/click Steam Play button")

    # More robust than Steam UI coordinates; used as the fallback for auto mode.
    log("launch: using steam://rungameid/730")
    run(["xdg-open", "steam://rungameid/730"], check=False)


def try_click_steam_play(args: argparse.Namespace, art: Path) -> bool:
    run(["xdg-open", "steam://nav/games/details/730"], check=False)
    time.sleep(args.steam_nav_wait)
    shot = screenshot(art / "screens" / f"steam-play-{int(time.time())}.png")
    center = find_steam_play_button(shot, args)
    if center is None:
        log("launch: did not find Steam Play button in screenshot")
        return False
    click(*center)
    return True


def find_steam_play_button(path: Path, args: argparse.Namespace) -> tuple[int, int] | None:
    # Prefer color detection. OCR can match unrelated "PLAY" text in Steam's
    # nav/header, while the actual launch button is a green rectangle.
    img = Image.open(path).convert("RGB")
    width, height = img.size
    max_x = min(width, args.left_width)
    max_y = min(height, args.left_height)
    xs: list[int] = []
    ys: list[int] = []
    for y in range(80, min(max_y, 320)):
        for x in range(80, min(max_x, 560)):
            r, g, b = img.getpixel((x, y))
            if g > 115 and g > r * 1.55 and g > b * 1.35 and (g - max(r, b)) > 55:
                xs.append(x)
                ys.append(y)
    if len(xs) >= 300:
        return (round((min(xs) + max(xs)) / 2), round((min(ys) + max(ys)) / 2))

    return find_ocr_word(path, r"PLAY")


def set_satur8(mode: str, saturation: float, art: Path, run_label: str) -> None:
    if mode == "off":
        run(["satur8", "off"], check=False, capture=True)
    else:
        run(["satur8", "on", f"{saturation:.2f}"], check=False, capture=True)
    loaded = run(
        ["qdbus6", "org.kde.KWin", "/Effects", "org.kde.kwin.Effects.isEffectLoaded", "satur8"],
        check=False,
        capture=True,
    )
    status = run(["satur8", "status"], check=False, capture=True)
    (art / f"{run_label}-satur8-state.txt").write_text(
        f"effect_loaded={loaded.stdout.strip()}\n\n{status.stdout}",
    )
    if mode == "off" and loaded.stdout.strip() != "false":
        raise RuntimeError("Satur8 effect is still loaded after satur8 off")
    if mode != "off":
        sat = run(
            [
                "qdbus6",
                "org.kde.KWin",
                "/org/kde/KWin/Effect/Satur81",
                "org.kde.kwin.Effect.Satur8.saturation",
            ],
            check=False,
            capture=True,
        )
        (art / f"{run_label}-saturation-readback.txt").write_text(sat.stdout + sat.stderr)
        if loaded.stdout.strip() != "true":
            raise RuntimeError("Satur8 effect did not load")


def capture_telemetry(art: Path, label: str) -> None:
    script = r'''
printf 'timestamp=%s\n' "$(date -Is)"
printf 'label=%s\n' "$LABEL"
pid=$(pgrep -n cs2 || true)
printf 'cs2_pid=%s\n' "${pid:-}"
if [ -n "${pid:-}" ]; then
  ps -o pid,etimes,rss,vsz,pmem,pcpu,cmd -p "$pid"
  awk '/VmRSS|VmHWM|VmSize|RssAnon|RssFile|VmSwap/ {print}' /proc/$pid/status
fi
for f in /sys/class/drm/card*/device/mem_info_vram_used /sys/class/drm/card*/device/mem_info_vram_total /sys/class/drm/card*/device/gpu_busy_percent; do
  [ -e "$f" ] && printf '%s=' "$f" && cat "$f"
done
command -v sensors >/dev/null && sensors 2>/dev/null | sed -n '/amdgpu-pci/,/^$/p;/coretemp-isa/,/^$/p'
'''
    env = os.environ.copy()
    env["LABEL"] = label
    proc = subprocess.run(["bash", "-c", script], text=True, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    (art / f"telemetry-{label}.txt").write_text(proc.stdout)


def navigate_to_workshop_and_click_go(args: argparse.Namespace, art: Path, run_label: str) -> None:
    deadline = time.monotonic() + args.menu_timeout
    last_shot = None
    while time.monotonic() < deadline:
        shot = screenshot(art / "screens" / f"{run_label}-menu-{int(time.time())}.png")
        last_shot = shot
        left_path = crop_left_to_file(shot, art / "screens" / f"{run_label}-menu-left.png", args.left_width, args.left_height)
        img = Image.open(left_path).convert("RGB")
        go = find_green_go_button(img)
        if go:
            click(go[0], go[1])
            return

        text = ocr_image(left_path)
        if re.search(r"\bPLAY\b", text, re.I):
            click(round(args.left_width * 0.492), round(args.left_height * 0.017))
            time.sleep(0.5)
            click(round(args.left_width * 0.543), round(args.left_height * 0.039))
            time.sleep(1.0)
            # The benchmark map should remain selected. If GO is still missing,
            # click the first workshop tile where the benchmark lives on PC1.
            shot2 = screenshot(art / "screens" / f"{run_label}-workshop-after-tabs.png")
            left2 = crop_left_to_file(shot2, art / "screens" / f"{run_label}-workshop-after-tabs-left.png", args.left_width, args.left_height)
            img2 = Image.open(left2).convert("RGB")
            go2 = find_green_go_button(img2)
            if go2:
                click(go2[0], go2[1])
                return
            click(round(args.left_width * 0.090), round(args.left_height * 0.130))
            time.sleep(0.8)
            shot3 = screenshot(art / "screens" / f"{run_label}-after-tile-click.png")
            left3 = crop_left_to_file(shot3, art / "screens" / f"{run_label}-after-tile-click-left.png", args.left_width, args.left_height)
            go3 = find_green_go_button(Image.open(left3).convert("RGB"))
            if go3:
                click(go3[0], go3[1])
                return
        time.sleep(2)

    raise RuntimeError(f"could not find CS2 GO button before timeout; last screenshot: {last_shot}")


def ensure_console_open(args: argparse.Namespace, art: Path, run_label: str) -> None:
    shot = screenshot(art / "screens" / f"{run_label}-console-check.png")
    left = crop_left_to_file(shot, art / "screens" / f"{run_label}-console-check-left.png", args.left_width, args.left_height)
    text = ocr_image(left)
    if "CONSOLE" not in text.upper():
        ydotool_key(f"{KEY_MINUS}:1", f"{KEY_MINUS}:0")
        time.sleep(1.0)


def copy_console_text(args: argparse.Namespace, art: Path, run_label: str) -> str:
    ensure_console_open(args, art, run_label)
    run(["wl-copy"], input_text="", check=False)
    time.sleep(0.1)
    ctrl_key(KEY_A)
    time.sleep(0.1)
    ctrl_key(KEY_C)
    time.sleep(0.4)
    clip = run(["wl-paste", "-n"], check=False, capture=True).stdout or ""
    if parse_vprof(clip) is not None:
        return clip

    shot = screenshot(art / "screens" / f"{run_label}-console-copy-fallback.png")
    left = crop_left_to_file(shot, art / "screens" / f"{run_label}-console-copy-fallback-left.png", args.left_width, args.left_height)
    text = ocr_image(left)
    return text


def wait_for_benchmark_result(args: argparse.Namespace, art: Path, run_label: str) -> str:
    start = time.monotonic()
    deadline = start + args.result_timeout
    while time.monotonic() < deadline:
        elapsed = time.monotonic() - start
        if elapsed < args.min_run_seconds:
            time.sleep(args.poll_seconds)
            continue
        text = copy_console_text(args, art, run_label)
        if parse_vprof(text) is not None:
            return text
        time.sleep(args.poll_seconds)
    raise RuntimeError(f"timed out waiting for VProf result for {run_label}")


def type_quit_and_wait(args: argparse.Namespace, art: Path, run_label: str) -> None:
    ensure_console_open(args, art, run_label)
    ydotool_type("quit")
    ydotool_key(f"{KEY_ENTER}:1", f"{KEY_ENTER}:0")
    if not wait_for_cs2_exit(args.quit_timeout):
        raise RuntimeError("CS2 did not exit after typing quit")


def parse_row_metric(text: str, label: str) -> tuple[float, float] | tuple[None, None]:
    # First two numeric columns after the label are All Frames Avg/P99.
    m = re.search(rf"{re.escape(label)}\s+([0-9]+(?:\.[0-9]+)?)\s+([0-9]+(?:\.[0-9]+)?)", text, re.I)
    if not m:
        return (None, None)
    return (float(m.group(1)), float(m.group(2)))


def parse_vprof(text: str) -> dict[str, object] | None:
    summary = re.search(
        r"Summary\s+of\s+([0-9]+)\s+frames\s+and\s+([0-9]+)\s+1-second\s+intervals\.\s+\(([0-9]+)\s+frames\s+excluded",
        text,
        re.I,
    )
    fps = re.search(r"FPS:\s*Avg=([0-9]+(?:\.[0-9]+)?),\s*P1=([0-9]+(?:\.[0-9]+)?)", text, re.I)
    if not summary or not fps:
        return None
    ft_avg, ft_p99 = parse_row_metric(text, "FrameTotal")
    cr_avg, cr_p99 = parse_row_metric(text, "Client Rendering")
    pr_avg, pr_p99 = parse_row_metric(text, "Present_RenderDevice")
    return {
        "frames_total": int(summary.group(1)),
        "one_second_intervals": int(summary.group(2)),
        "frames_excluded": int(summary.group(3)),
        "avg_fps": float(fps.group(1)),
        "p1_fps": float(fps.group(2)),
        "frame_total_avg_ms": ft_avg,
        "frame_total_p99_ms": ft_p99,
        "client_rendering_avg_ms": cr_avg,
        "client_rendering_p99_ms": cr_p99,
        "present_renderdevice_avg_ms": pr_avg,
        "present_renderdevice_p99_ms": pr_p99,
    }


def validate(parsed: dict[str, object], args: argparse.Namespace) -> list[str]:
    reasons: list[str] = []
    if int(parsed["frames_excluded"]) != 0:
        reasons.append(f"frames_excluded={parsed['frames_excluded']}")
    if int(parsed["one_second_intervals"]) != args.expected_intervals:
        reasons.append(f"one_second_intervals={parsed['one_second_intervals']}")
    return reasons


def append_result(
    art: Path,
    result: RunResult,
    *,
    accepted_path: Path,
    rejected_path: Path,
) -> None:
    fields = [
        "test_id",
        "timestamp",
        "game",
        "map",
        "mode",
        "saturation",
        "pair_index",
        "attempt_index",
        "sequence_index",
        "accepted",
        "rejection_reasons",
        "avg_fps",
        "p1_fps",
        "frames_total",
        "frames_excluded",
        "one_second_intervals",
        "frame_total_avg_ms",
        "frame_total_p99_ms",
        "client_rendering_avg_ms",
        "client_rendering_p99_ms",
        "present_renderdevice_avg_ms",
        "present_renderdevice_p99_ms",
        "raw_console_text_path",
    ]
    row = {
        "test_id": art.name,
        "timestamp": result.timestamp,
        "game": "Counter-Strike 2",
        "map": "CS2 FPS BENCHMARK DUST2 / de_dust2",
        "mode": result.mode,
        "saturation": result.saturation,
        "pair_index": result.pair_index,
        "attempt_index": result.attempt_index,
        "sequence_index": result.sequence_index,
        "accepted": result.accepted,
        "rejection_reasons": ";".join(result.rejection_reasons),
        "avg_fps": result.avg_fps,
        "p1_fps": result.p1_fps,
        "frames_total": result.frames_total,
        "frames_excluded": result.frames_excluded,
        "one_second_intervals": result.one_second_intervals,
        "frame_total_avg_ms": result.frame_total_avg_ms,
        "frame_total_p99_ms": result.frame_total_p99_ms,
        "client_rendering_avg_ms": result.client_rendering_avg_ms,
        "client_rendering_p99_ms": result.client_rendering_p99_ms,
        "present_renderdevice_avg_ms": result.present_renderdevice_avg_ms,
        "present_renderdevice_p99_ms": result.present_renderdevice_p99_ms,
        "raw_console_text_path": result.raw_console_text_path,
    }
    target = accepted_path if result.accepted else rejected_path
    write_header = not target.exists()
    with target.open("a", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=fields)
        if write_header:
            writer.writeheader()
        writer.writerow(row)
    jsonl = target.with_suffix(".jsonl")
    with jsonl.open("a") as f:
        f.write(json.dumps(row, sort_keys=True) + "\n")


def summarize(art: Path) -> None:
    path = art / "runs.csv"
    if not path.exists():
        return
    rows = list(csv.DictReader(path.open()))
    for row in rows:
        for key in ("avg_fps", "p1_fps"):
            row[key] = float(row[key])
    by_mode = {mode: [r for r in rows if r["mode"] == mode] for mode in ("off", "kwin")}
    if not by_mode["off"] or not by_mode["kwin"]:
        return
    def med(mode: str, key: str) -> float:
        values = sorted(float(r[key]) for r in by_mode[mode])
        n = len(values)
        return values[n // 2] if n % 2 else (values[n // 2 - 1] + values[n // 2]) / 2
    summary = {
        "test_id": art.name,
        "updated_at": datetime.now(timezone.utc).isoformat(),
        "accepted_runs": len(rows),
        "off_n": len(by_mode["off"]),
        "kwin_n": len(by_mode["kwin"]),
        "off_avg_fps_median": med("off", "avg_fps"),
        "kwin_avg_fps_median": med("kwin", "avg_fps"),
        "off_p1_fps_median": med("off", "p1_fps"),
        "kwin_p1_fps_median": med("kwin", "p1_fps"),
    }
    summary["avg_fps_delta"] = summary["kwin_avg_fps_median"] - summary["off_avg_fps_median"]
    summary["avg_frame_time_delta_ms"] = (
        1000 / summary["kwin_avg_fps_median"] - 1000 / summary["off_avg_fps_median"]
    )
    (art / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")


def run_one_attempt(
    args: argparse.Namespace,
    art: Path,
    *,
    mode: str,
    pair_index: int,
    attempt_index: int,
    sequence_index: int,
) -> RunResult:
    saturation = 1.0 if mode == "off" else args.saturation
    run_label = f"pair{pair_index:02d}-{mode}-attempt{attempt_index:02d}"
    capture_telemetry(art, f"before-{run_label}")
    set_satur8(mode, args.saturation, art, run_label)

    if not wait_for_cs2_process(2):
        launch_cs2(args, art)
        log("launch: waiting for CS2 process")
        if not wait_for_cs2_process(args.launch_timeout):
            raise RuntimeError("CS2 did not launch")
    log("launch: CS2 process is running")
    time.sleep(args.after_launch_wait)

    log(f"{run_label}: navigating to benchmark and clicking GO")
    navigate_to_workshop_and_click_go(args, art, run_label)
    log(f"{run_label}: waiting for VProf result")
    text = wait_for_benchmark_result(args, art, run_label)

    raw_path = art / "raw-console" / f"{run_label}.txt"
    raw_path.parent.mkdir(parents=True, exist_ok=True)
    raw_path.write_text(text)

    parsed = parse_vprof(text)
    if parsed is None:
        raise RuntimeError(f"failed to parse VProf output for {run_label}")
    reasons = validate(parsed, args)
    accepted = not reasons

    result = RunResult(
        mode=mode,
        saturation=saturation,
        pair_index=pair_index,
        attempt_index=attempt_index,
        sequence_index=sequence_index,
        accepted=accepted,
        rejection_reasons=reasons,
        raw_console_text_path=str(raw_path),
        timestamp=datetime.now(timezone.utc).isoformat(),
        avg_fps=float(parsed["avg_fps"]),
        p1_fps=float(parsed["p1_fps"]),
        frames_total=int(parsed["frames_total"]),
        frames_excluded=int(parsed["frames_excluded"]),
        one_second_intervals=int(parsed["one_second_intervals"]),
        frame_total_avg_ms=parsed["frame_total_avg_ms"],
        frame_total_p99_ms=parsed["frame_total_p99_ms"],
        client_rendering_avg_ms=parsed["client_rendering_avg_ms"],
        client_rendering_p99_ms=parsed["client_rendering_p99_ms"],
        present_renderdevice_avg_ms=parsed["present_renderdevice_avg_ms"],
        present_renderdevice_p99_ms=parsed["present_renderdevice_p99_ms"],
    )
    append_result(art, result, accepted_path=art / "runs.csv", rejected_path=art / "rejected-runs.csv")
    summarize(art)
    capture_telemetry(art, f"after-{run_label}")
    log(f"{run_label}: quitting CS2")
    type_quit_and_wait(args, art, run_label)
    return result


def pair_order(pair_index: int, args: argparse.Namespace) -> list[str]:
    if args.order == "off-on":
        return ["off", "kwin"]
    if args.order == "on-off":
        return ["kwin", "off"]
    return ["off", "kwin"] if pair_index % 2 else ["kwin", "off"]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pairs", type=int, default=7, help="accepted pairs to collect")
    parser.add_argument("--saturation", type=float, default=1.75)
    parser.add_argument("--order", choices=["flip", "off-on", "on-off"], default="flip")
    parser.add_argument("--expected-intervals", type=int, default=129)
    parser.add_argument("--max-attempts-per-slot", type=int, default=4)
    parser.add_argument("--left-width", type=int, default=2560)
    parser.add_argument("--left-height", type=int, default=1440)
    parser.add_argument("--steam-launch", choices=["auto", "uri", "click"], default="auto")
    parser.add_argument("--steam-nav-wait", type=float, default=4.0)
    parser.add_argument("--click-launch-grace", type=int, default=20)
    parser.add_argument("--launch-timeout", type=int, default=180)
    parser.add_argument("--quit-timeout", type=int, default=60)
    parser.add_argument("--menu-timeout", type=int, default=180)
    parser.add_argument("--after-launch-wait", type=float, default=18.0)
    parser.add_argument("--min-run-seconds", type=int, default=115)
    parser.add_argument("--result-timeout", type=int, default=240)
    parser.add_argument("--poll-seconds", type=float, default=5.0)
    parser.add_argument("--artifact-dir", type=Path)
    args = parser.parse_args()

    require_tools(["ydotool", "spectacle", "tesseract", "wl-copy", "wl-paste", "xdg-open", "satur8", "qdbus6"])

    test_id = args.artifact_dir or BENCH_ROOT / f"cs2-kwin-restart-{datetime.now().strftime('%Y%m%d-%H%M%S')}"
    art = test_id.resolve()
    art.mkdir(parents=True, exist_ok=True)
    (art / "session.json").write_text(
        json.dumps(
            {
                "created_at": datetime.now(timezone.utc).isoformat(),
                "protocol": "restart CS2 every attempt; accept only clean VProf runs",
                "pairs": args.pairs,
                "saturation": args.saturation,
                "order": args.order,
                "expected_intervals": args.expected_intervals,
            },
            indent=2,
            sort_keys=True,
        )
        + "\n"
    )

    accepted = {("off", i): 0 for i in range(1, args.pairs + 1)}
    accepted.update({("kwin", i): 0 for i in range(1, args.pairs + 1)})
    attempts = {key: 0 for key in accepted}
    sequence = 0

    for pair_index in range(1, args.pairs + 1):
        for mode in pair_order(pair_index, args):
            key = (mode, pair_index)
            while accepted[key] < 1:
                attempts[key] += 1
                if attempts[key] > args.max_attempts_per_slot:
                    raise RuntimeError(f"too many dirty/failed attempts for pair {pair_index} {mode}")
                sequence += 1
                result = run_one_attempt(
                    args,
                    art,
                    mode=mode,
                    pair_index=pair_index,
                    attempt_index=attempts[key],
                    sequence_index=sequence,
                )
                state = "accepted" if result.accepted else f"rejected ({', '.join(result.rejection_reasons)})"
                print(
                    f"{state}: pair={pair_index} mode={mode} avg={result.avg_fps} p1={result.p1_fps} "
                    f"excluded={result.frames_excluded} intervals={result.one_second_intervals}",
                    flush=True,
                )
                if result.accepted:
                    accepted[key] = 1

    run(["satur8", "off"], check=False)
    print(f"done: {art}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
