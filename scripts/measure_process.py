#!/usr/bin/env python3
"""Repeatable black-box Oikade process measurements using only the stdlib."""

import argparse
import datetime as dt
import json
import math
import os
import pathlib
import signal
import statistics
import subprocess
import sys
import threading
import time

MAX_OUTPUT = 1 << 20


def duration(value: str) -> float:
    units = {"ms": 0.001, "s": 1.0, "m": 60.0, "h": 3600.0}
    for suffix in ("ms", "s", "m", "h"):
        if value.endswith(suffix):
            result = float(value[: -len(suffix)]) * units[suffix]
            if not math.isfinite(result) or result < 0:
                raise ValueError
            return result
    result = float(value)
    if not math.isfinite(result) or result < 0:
        raise ValueError
    return result


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    parser.add_argument("--scenario", required=True)
    parser.add_argument("--ready-token", required=True)
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--warmup", type=duration, default=3.0)
    parser.add_argument("--idle-duration", type=duration, default=60.0)
    parser.add_argument("--sample-interval", type=duration, default=1.0)
    parser.add_argument("--startup-timeout", type=duration, default=10.0)
    parser.add_argument("--shutdown-timeout", type=duration, default=10.0)
    parser.add_argument("--sidecar-binary")
    parser.add_argument("--include-samples", action="store_true")
    parser.add_argument("--stimulus-binary")
    parser.add_argument("--stimulus-arg", action="append", default=[])
    parser.add_argument("--stimulus-interval", type=duration, default=0.25)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    result = parser.parse_args()
    if result.command[:1] == ["--"]:
        result.command = result.command[1:]
    if result.runs < 1 or result.idle_duration <= 0 or result.sample_interval <= 0:
        parser.error("runs and sampling durations must be positive")
    if not pathlib.Path(result.binary).is_absolute():
        parser.error("--binary must be absolute")
    if result.sidecar_binary and not pathlib.Path(result.sidecar_binary).is_absolute():
        parser.error("--sidecar-binary must be absolute")
    if bool(result.stimulus_binary) != bool(result.stimulus_arg):
        parser.error("--stimulus-binary and at least one --stimulus-arg are required together")
    return result


class Output:
    def __init__(self, token: str):
        self.token = token
        self.buffer = ""
        self.ready_at = None
        self.ready = threading.Event()
        self.lock = threading.Lock()

    def consume(self, stream):
        for raw in iter(stream.readline, b""):
            text = raw.decode("utf-8", errors="replace")
            with self.lock:
                self.buffer = (self.buffer + text)[-MAX_OUTPUT:]
                if self.ready_at is None and self.token in self.buffer:
                    self.ready_at = time.monotonic()
                    self.ready.set()

    def text(self) -> str:
        with self.lock:
            return self.buffer


def cpu_seconds(value: str) -> float:
    days = 0
    if "-" in value:
        day, value = value.split("-", 1)
        days = int(day)
    parts = value.split(":")
    if len(parts) == 2:
        hours, minutes, seconds = 0, int(parts[0]), float(parts[1])
    elif len(parts) == 3:
        hours, minutes, seconds = int(parts[0]), int(parts[1]), float(parts[2])
    else:
        raise ValueError(f"unsupported ps CPU time {value!r}")
    return days * 86400 + hours * 3600 + minutes * 60 + seconds


def process_table():
    output = subprocess.check_output(
        ["ps", "-axo", "pid=,ppid=,rss=,time=,command="], text=True
    )
    table = {}
    for line in output.splitlines():
        fields = line.strip().split(None, 4)
        if len(fields) == 5:
            table[int(fields[0])] = {
                "ppid": int(fields[1]),
                "rss": int(fields[2]) * 1024,
                "cpu": cpu_seconds(fields[3]),
                "command": fields[4],
            }
    return table


def descendant(table, root: int, executable: str):
    wanted = str(pathlib.Path(executable).resolve())
    descendants = {root}
    changed = True
    while changed:
        changed = False
        for pid, entry in table.items():
            if entry["ppid"] in descendants and pid not in descendants:
                descendants.add(pid)
                changed = True
    for pid in descendants - {root}:
        command = table[pid]["command"]
        token = command.split(None, 1)[0] if command else ""
        try:
            candidate = str(pathlib.Path(token).resolve())
        except OSError:
            candidate = token
        if candidate == wanted:
            return pid
    raise RuntimeError(f"sidecar process {wanted!r} was not found below PID {root}")


def sample(pid: int, sidecar: int | None):
    table = process_table()
    if pid not in table:
        raise RuntimeError(f"process {pid} exited during sampling")
    point = {
        "measured_at": time.monotonic(),
        "rss_bytes": table[pid]["rss"],
        "cpu_seconds": table[pid]["cpu"],
    }
    if sidecar is not None:
        if sidecar not in table:
            raise RuntimeError(f"sidecar process {sidecar} exited during sampling")
        point["sidecar_rss_bytes"] = table[sidecar]["rss"]
        point["sidecar_cpu_seconds"] = table[sidecar]["cpu"]
    return point


def stimulate(config, stopped: threading.Event, failures: list[str]):
    value = False
    while not stopped.wait(config.stimulus_interval):
        value = not value
        command = [config.stimulus_binary] + [
            argument.replace("{toggle}", str(value).lower())
            for argument in config.stimulus_arg
        ]
        result = subprocess.run(
            command, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True
        )
        if result.returncode != 0:
            failures.append(result.stderr.strip() or f"stimulus exited {result.returncode}")
            stopped.set()
            return


def percentile(values, fraction):
    ordered = sorted(values)
    return ordered[max(0, math.ceil(len(ordered) * fraction) - 1)]


def measure(config, run_number: int):
    output = Output(config.ready_token)
    started = time.monotonic()
    process = subprocess.Popen(
        [config.binary] + config.command,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )
    reader = threading.Thread(target=output.consume, args=(process.stdout,), daemon=True)
    reader.start()
    try:
        if not output.ready.wait(config.startup_timeout):
            if process.poll() is not None:
                raise RuntimeError(f"process exited before readiness: {output.text()}")
            raise RuntimeError(f"readiness timed out: {output.text()}")
        startup_ms = (output.ready_at - started) * 1000
        time.sleep(config.warmup)
        if process.poll() is not None:
            raise RuntimeError(f"process exited during warmup: {output.text()}")
        table = process_table()
        sidecar = (
            descendant(table, process.pid, config.sidecar_binary)
            if config.sidecar_binary
            else None
        )
        stop_stimulus = threading.Event()
        failures = []
        stimulus = None
        if config.stimulus_binary:
            stimulus = threading.Thread(
                target=stimulate,
                args=(config, stop_stimulus, failures),
                daemon=True,
            )
            stimulus.start()
        sampling_started = time.monotonic()
        samples = [sample(process.pid, sidecar)]
        while time.monotonic() - sampling_started < config.idle_duration:
            time.sleep(min(config.sample_interval, config.idle_duration))
            if failures:
                raise RuntimeError(f"stimulus failed: {failures[0]}")
            samples.append(sample(process.pid, sidecar))
        stop_stimulus.set()
        if stimulus:
            stimulus.join(timeout=2)
        shutdown_started = time.monotonic()
        os.killpg(process.pid, signal.SIGINT)
        process.wait(timeout=config.shutdown_timeout)
        if process.returncode != 0:
            raise RuntimeError(f"process exited {process.returncode}: {output.text()}")
        result = summarize_run(run_number, startup_ms, samples)
        result["shutdown_ms"] = (time.monotonic() - shutdown_started) * 1000
        if config.include_samples:
            initial = samples[0]["measured_at"]
            result["sample_series"] = [
                {
                    "elapsed_ms": (entry["measured_at"] - initial) * 1000,
                    **{key: value for key, value in entry.items() if key != "measured_at"},
                }
                for entry in samples
            ]
        return result
    finally:
        if process.poll() is None:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait()


def summarize_run(number, startup_ms, samples):
    elapsed = samples[-1]["measured_at"] - samples[0]["measured_at"]
    cpu = samples[-1]["cpu_seconds"] - samples[0]["cpu_seconds"]
    result = {
        "run": number,
        "startup_ms": startup_ms,
        "idle_ms": elapsed * 1000,
        "mean_cpu_percent": cpu / elapsed * 100 if elapsed else 0,
        "peak_rss_bytes": max(point["rss_bytes"] for point in samples),
        "rss_delta_bytes": samples[-1]["rss_bytes"] - samples[0]["rss_bytes"],
        "samples": len(samples),
    }
    if "sidecar_rss_bytes" in samples[0]:
        sidecar_cpu = samples[-1]["sidecar_cpu_seconds"] - samples[0]["sidecar_cpu_seconds"]
        result.update({
            "sidecar_mean_cpu_percent": sidecar_cpu / elapsed * 100 if elapsed else 0,
            "sidecar_peak_rss_bytes": max(point["sidecar_rss_bytes"] for point in samples),
            "sidecar_rss_delta_bytes": samples[-1]["sidecar_rss_bytes"] - samples[0]["sidecar_rss_bytes"],
        })
    return result


def main():
    config = arguments()
    runs = [measure(config, number) for number in range(1, config.runs + 1)]
    summary = {
        "startup_median_ms": statistics.median(run["startup_ms"] for run in runs),
        "startup_p95_ms": percentile([run["startup_ms"] for run in runs], 0.95),
        "mean_cpu_percent": statistics.mean(run["mean_cpu_percent"] for run in runs),
        "peak_rss_bytes": max(run["peak_rss_bytes"] for run in runs),
        "maximum_rss_growth_bytes": max(run["rss_delta_bytes"] for run in runs),
        "shutdown_p95_ms": percentile([run["shutdown_ms"] for run in runs], 0.95),
    }
    if config.sidecar_binary:
        summary.update({
            "sidecar_mean_cpu_percent": statistics.mean(run["sidecar_mean_cpu_percent"] for run in runs),
            "sidecar_peak_rss_bytes": max(run["sidecar_peak_rss_bytes"] for run in runs),
            "maximum_sidecar_rss_growth_bytes": max(run["sidecar_rss_delta_bytes"] for run in runs),
        })
    json.dump({
        "schema_version": 2,
        "scenario": config.scenario,
        "binary": config.binary,
        "arguments": config.command,
        "measured_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "runs": runs,
        "summary": summary,
    }, sys.stdout, indent=2)
    print()


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"measure-process: {error}", file=sys.stderr)
        raise SystemExit(1)
