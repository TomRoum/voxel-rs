import csv
import json
import platform
import signal
import subprocess
import time
from datetime import datetime, timezone


def command_output(command):
    try:
        return subprocess.check_output(command, text=True, stderr=subprocess.DEVNULL).strip()
    except (OSError, subprocess.CalledProcessError):
        return None


def system_survey():
    powershell = [
        "powershell",
        "-NoProfile",
        "-Command",
        "Get-CimInstance Win32_ComputerSystem | Select-Object Manufacturer,Model,TotalPhysicalMemory | ConvertTo-Json -Compress",
    ]
    computer = {}
    try:
        computer = json.loads(subprocess.check_output(powershell, text=True, stderr=subprocess.DEVNULL))
    except (OSError, subprocess.CalledProcessError, json.JSONDecodeError):
        pass

    graphics_command = powershell[:-1] + [
        "Get-CimInstance Win32_VideoController | Select-Object Name,DriverVersion,AdapterRAM,CurrentHorizontalResolution,CurrentVerticalResolution,CurrentRefreshRate | ConvertTo-Json -Compress",
    ]
    try:
        graphics = json.loads(subprocess.check_output(graphics_command, text=True, stderr=subprocess.DEVNULL))
        if isinstance(graphics, dict):
            graphics = [graphics]
    except (OSError, subprocess.CalledProcessError, json.JSONDecodeError):
        graphics = []

    return {
        "timestamp_utc": datetime.now(timezone.utc).isoformat(),
        "os": platform.platform(),
        "python": platform.python_version(),
        "cpu": platform.processor(),
        "machine": platform.machine(),
        "computer": computer,
        "graphics": graphics,
        "git_commit": command_output(["git", "rev-parse", "HEAD"]),
    }


def performance_snapshot(pid):
    command = [
        "powershell",
        "-NoProfile",
        "-Command",
        "$p = Get-Process -Id " + str(pid) + " -ErrorAction SilentlyContinue; "
        "$cpu = (Get-Counter '\\Processor(_Total)\\% Processor Time' -ErrorAction SilentlyContinue).CounterSamples.CookedValue; "
        "$gpu = (Get-Counter '\\GPU Engine(*)\\Utilization Percentage' -ErrorAction SilentlyContinue).CounterSamples.CookedValue; "
        "$gpu_util = $null; if ($gpu) { $gpu_util = ($gpu | Measure-Object -Maximum).Maximum }; "
        "[pscustomobject]@{cpu_time_s=$p.CPU; working_set_mb=$p.WorkingSet64 / 1MB; cpu_utilization_percent=$cpu; gpu_utilization_percent=$gpu_util} | ConvertTo-Json -Compress",
    ]
    try:
        return json.loads(subprocess.check_output(command, text=True, stderr=subprocess.DEVNULL))
    except (OSError, subprocess.CalledProcessError, json.JSONDecodeError):
        return {}


def performance_samples(pid, duration_s=20, interval_s=1):
    samples = []
    started = time.monotonic()
    deadline = started + duration_s
    while time.monotonic() < deadline:
        sample = performance_snapshot(pid)
        if sample:
            samples.append((time.monotonic(), sample))
        next_sample = started + (len(samples) * interval_s)
        time.sleep(max(0, min(next_sample - time.monotonic(), deadline - time.monotonic())))

    fields = ("cpu_time_s", "working_set_mb", "cpu_utilization_percent", "gpu_utilization_percent")
    summary = {
        "sample_count": len(samples),
        "requested_interval_s": interval_s,
        "duration_s": min(time.monotonic(), deadline) - started,
    }
    for field in fields:
        values = [sample[field] for _, sample in samples if sample.get(field) is not None]
        if values:
            summary[field] = {
                "avg": sum(values) / len(values),
                "min": min(values),
                "max": max(values),
                "sample_count": len(values),
            }
        else:
            summary[field] = None
    if len(samples) > 1:
        summary["effective_interval_s"] = (samples[-1][0] - samples[0][0]) / (len(samples) - 1)
    else:
        summary["effective_interval_s"] = None
    return summary


def run_benchmark(svo_type, render_distance, render_shadows, no_lod):
    cmd = [
        "cargo",
        "run",
        "--release",
        "--features=benchmark," + "use-" + svo_type,
        "--no-default-features",
        "--",
        "--pos", "-644", "97", "120",
        "--rot", "-1", "165", "0",
        "--detach-input",
        "--render-distance=" + str(render_distance),
        "--fov=80",
        "--mc-world=assets/worlds/benchmark",
        "--render-shadows=" + ("true" if render_shadows else "false"),
        "--no-lod=" + ("true" if no_lod else "false"),
        "--gpu-buffer-size=3000"
    ]

    started = time.monotonic()
    started_utc = datetime.now(timezone.utc).isoformat()
    process = subprocess.Popen(cmd, shell=False, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                               creationflags=subprocess.CREATE_NEW_PROCESS_GROUP)
    assert process.stdout is not None
    output_lines = []
    loaded_at = None

    while True:
        output = process.stdout.readline().rstrip().decode("utf-8", errors="replace")
        output_lines.append(output)
        if output == "" and process.poll() is not None:
            break
        if output == "all chunks loaded":
            loaded_at = time.monotonic()
            break

    performance = performance_samples(process.pid)

    if process.poll() is None:
        process.send_signal(signal.CTRL_BREAK_EVENT)
    process.wait()

    try:
        remaining, _ = process.communicate(timeout=2)
    except subprocess.TimeoutExpired:
        process.terminate()
        remaining, _ = process.communicate()

    output_lines.extend(remaining.decode("utf-8", errors="replace").splitlines())
    benchmark = None
    for line in output_lines:
        prefix = "benchmark: "
        if line.startswith(prefix):
            benchmark = json.loads(line[len(prefix):])
            break

    if benchmark is not None:
        benchmark["survey"] = {
            "started_utc": started_utc,
            "process_exit_code": process.returncode,
            "process_duration_s": time.monotonic() - started,
            "time_to_all_chunks_loaded_s": None if loaded_at is None else loaded_at - started,
            "performance": performance,
        }
        return benchmark

    return None


def unique_combinations(matrix):
    results = []

    for key, values in matrix.items():
        if len(results) == 0:
            for v in values:
                results.append({key: v})
            continue

        new_results = []
        for v in values:
            for r in results:
                copy = r.copy()
                copy[key] = v
                new_results.append(copy)
        results = new_results

    return results


def nested_keys(d: dict, path=""):
    for k, v in d.items():
        if type(v) is dict:
            yield from nested_keys(v, path=path + k + ".")
        else:
            yield path + k, v


def main():
    matrix = {
        "render_distance": [30],
        "render_shadows": [True, False],
        "no_lod": [True, False],
        # put svo type last so that build cache is only discarded once
        "svo_type": ["esvo", "csvo"],
    }

    results = []
    for i, case in enumerate(unique_combinations(matrix)):
        print("running case", case)
        r = run_benchmark(case["svo_type"], case["render_distance"], case["render_shadows"], case["no_lod"])
        results.append((case, r))

    if len(results) == 0:
        print("no results")
        return

    system = system_survey()
    with open("survey.json", "w", newline="") as f:
        json.dump({"system": system, "runs": len(results)}, f, indent=2)

    with open("results.csv", "w", newline="") as f:
        w = csv.writer(f)

        header = [k for k in matrix.keys()]
        header.extend(dict(nested_keys(results[0][1])).keys())
        w.writerow(header)

        for r in results:
            row = [v for v in r[0].values()]
            row.extend(dict(nested_keys(r[1])).values())
            w.writerow(row)


if __name__ == "__main__":
    main()
