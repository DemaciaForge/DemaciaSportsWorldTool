#!/usr/bin/env python3
"""Offline HAR/OBS contract audit. Only aggregate, allowlisted facts are emitted.

No network calls, account/session reads, decrypted payload export or replay.
Run `python tools/audit_run_capture.py --help` for input/companion options.
"""

import argparse
import base64
import collections
import copy
import datetime as dt
import gzip
import hashlib
import io
import json
import math
from pathlib import Path
import sys
import zlib
from urllib.parse import urlsplit

MAX_FILE = 32 * 1024 * 1024
MAX_FIELD = 8 * 1024 * 1024
OBS_FIELDS = frozenset((
    "rrid", "uuid", "uid", "run_data", "extension_json", "fixed_point_json",
    "segment_json", "speed_json", "step_freq_json", "laps_json", "runFaceCheck",
))
POINT_FIELDS = frozenset((
    "avgSpeed", "bdA", "bdD", "bdG", "bdS", "coorType", "count", "dtr", "flag",
    "gLat", "gLng", "gainTime", "id", "lat", "lng", "locType", "locationId",
    "queueNum", "radius", "speed", "state", "stepDistance", "totalDis",
    "totalTime", "type", "validDis", "validTime",
))


class AuditError(ValueError):
    """Messages must be fixed descriptions, without untrusted input values."""


def _unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise AuditError("duplicate JSON member")
        result[key] = value
    return result


def _nonfinite(_value):
    raise AuditError("nonfinite JSON number")


def _finite_float(value):
    number = float(value)
    if not math.isfinite(number):
        raise AuditError("nonfinite JSON number")
    return number


def parse_json(text):
    try:
        return json.loads(text, object_pairs_hook=_unique_object,
                          parse_constant=_nonfinite, parse_float=_finite_float)
    except (json.JSONDecodeError, RecursionError) as exc:
        raise AuditError("invalid JSON") from exc


def load_json(path):
    # Bound the read itself, not just a potentially stale stat().
    with Path(path).open("rb") as stream:
        data = stream.read(MAX_FILE + 1)
    if len(data) > MAX_FILE:
        raise AuditError("input exceeds size limit")
    try:
        return parse_json(data.decode("utf-8-sig")), hashlib.sha256(data).hexdigest()
    except UnicodeError as exc:
        raise AuditError("input is not UTF-8") from exc


def decode_obs(obj):
    if not isinstance(obj, dict) or not OBS_FIELDS.intersection(obj):
        raise AuditError("expected an encoded OBS object")
    decoded = {}
    for key in sorted(OBS_FIELDS.intersection(obj)):
        value = obj[key]
        if not isinstance(value, str) or len(value) > MAX_FIELD * 2:
            raise AuditError("invalid encoded OBS field")
        try:
            compressed = base64.b64decode(value, validate=True)
            with gzip.GzipFile(fileobj=io.BytesIO(compressed)) as stream:
                raw = stream.read(MAX_FIELD + 1)
            if len(raw) > MAX_FIELD:
                raise AuditError("decoded field exceeds size limit")
            decoded[key] = raw.decode("utf-8")
        except (ValueError, OSError, EOFError, UnicodeError, zlib.error) as exc:
            if isinstance(exc, AuditError):
                raise
            raise AuditError("invalid gzip/base64 OBS field") from exc
    return decoded


def _object(text):
    value = parse_json(text)
    if not isinstance(value, dict):
        raise AuditError("expected a JSON object")
    return value


def _array(value):
    if isinstance(value, str):
        value = parse_json(value) if value else []
    if not isinstance(value, list) or any(not isinstance(x, dict) for x in value):
        raise AuditError("expected an array of objects")
    return value


def _number(row, key):
    value = row.get(key)
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
        raise AuditError("missing or invalid numeric measurement")
    return value


def _distance(a, b):
    lat, lon, lat2, lon2 = map(math.radians, (*a, *b))
    h = math.sin((lat2 - lat) / 2) ** 2 + math.cos(lat) * math.cos(lat2) * math.sin((lon2 - lon) / 2) ** 2
    return 6371000 * 2 * math.asin(min(1, math.sqrt(h)))


def _bd_to_gcj(lat, lon):
    x, y, xpi = lon - 0.0065, lat - 0.006, math.pi * 3000 / 180
    z = math.hypot(x, y) - 0.00002 * math.sin(y * xpi)
    theta = math.atan2(y, x) - 0.000003 * math.cos(x * xpi)
    return z * math.sin(theta), z * math.cos(theta)


def summarize_obs(obj):
    decoded = decode_obs(obj)
    run = _object(decoded.get("run_data", "{}"))
    points = _array(run.get("allLocJson", []))
    if not points:
        raise AuditError("track has no points")
    speed = _array(decoded.get("speed_json", ""))
    steps = _array(decoded.get("step_freq_json", ""))
    laps = _array(decoded.get("laps_json", ""))
    fixed = _object(decoded.get("fixed_point_json", "{}"))
    checkpoints = _array(fixed.get("fivePointJson", []))
    times = [_number(p, "totalTime") for p in points]
    gaps = collections.Counter(b - a for a, b in zip(times, times[1:]))
    distances = [_number(p, "totalDis") for p in points]
    wall = []
    for point in points:
        try:
            wall.append(dt.datetime.fromisoformat(point["gainTime"]))
        except (KeyError, ValueError, TypeError) as exc:
            raise AuditError("invalid gainTime timestamp") from exc
    try:
        wall_span = (max(wall) - min(wall)).total_seconds()
    except TypeError as exc:
        raise AuditError("inconsistent gainTime timezone") from exc
    gps = [(_number(p, "gLat"), _number(p, "gLng")) for p in points]
    conversion_errors, nearest = [], []
    for p in checkpoints:
        gcj = (_number(p, "glat"), _number(p, "glon"))
        bd = (_number(p, "lat"), _number(p, "lon"))
        conversion_errors.append(round(_distance(_bd_to_gcj(*bd), gcj), 3))
        nearest.append(round(min(_distance(gcj, xy) for xy in gps), 3))
    point_keys = set.union(*(set(p) for p in points))
    speed_sum = sum(_number(p, "distance") for p in speed)
    step_sum = sum(_number(p, "stepsNum") for p in steps)
    durations = [(_number(p, "endTime") - _number(p, "beginTime")) / 1000 for p in speed]
    paired = len(speed) == len(steps) and all(
        all(k in a and k in b and a[k] == b[k] for k in ("id", "queueNum", "beginTime", "endTime"))
        for a, b in zip(speed, steps)
    )
    summary = {
        "field_count": len(obj), "missing_fields": sorted(OBS_FIELDS - obj.keys()),
        "unknown_field_count": len(obj.keys() - OBS_FIELDS),
        "empty_fields": sorted(k for k, v in decoded.items() if not v),
        "point_count": len(points), "point_fields": sorted(point_keys & POINT_FIELDS),
        "unknown_point_field_count": len(point_keys - POINT_FIELDS),
        "point_schema_uniform": all(set(p) == point_keys for p in points),
        "track_time_span_s": max(times) - min(times),
        "wall_time_first_last_s": (wall[-1] - wall[0]).total_seconds(),
        "wall_time_range_s": wall_span,
        "wall_time_gaps": [{"seconds": g, "count": c} for g, c in sorted(collections.Counter(
            (b - a).total_seconds() for a, b in zip(wall, wall[1:])).items())],
        "track_time_gaps": [{"seconds": g, "count": c} for g, c in sorted(gaps.items())],
        "track_distance_m": round(max(distances), 6),
        "speed_window_count": len(speed), "step_window_count": len(steps),
        "window_pairs_aligned": paired,
        "speed_window_duration_range_s": [min(durations), max(durations)] if durations else [],
        "speed_window_distance_sum_m": round(speed_sum, 6),
        "track_minus_speed_windows_m": round(max(distances) - speed_sum, 6),
        "step_window_sum": step_sum,
        "lap_count": len(laps), "lap_distance_sum_m": round(sum(_number(p, "distance") for p in laps), 6),
        "lap_duration_sum_s": sum(_number(p, "duration") for p in laps),
        "lap_step_sum": sum(_number(p, "step") for p in laps),
        "checkpoint_count": len(checkpoints),
        "checkpoint_bd09_to_gcj02_residual_m": conversion_errors,
        "checkpoint_nearest_recorded_point_m": nearest,
        "face_entry_count": len(_array(decoded.get("runFaceCheck", ""))),
    }
    warnings = []
    if summary["missing_fields"]:
        warnings.append("missing_obs_fields")
    if wall_span != summary["track_time_span_s"]:
        warnings.append("wall_and_track_clocks_differ")
    if any(g <= 0 for g in gaps):
        warnings.append("nonincreasing_track_time")
    if any(d <= 0 or d > 15 for d in durations):
        warnings.append("irregular_window_duration")
    if abs(max(distances) - speed_sum) > 0.01:
        warnings.append("window_distance_differs_from_track")
    if summary["lap_step_sum"] != step_sum:
        warnings.append("lap_steps_differ_from_windows")
    if not paired:
        warnings.append("window_pairs_not_aligned")
    summary["observations"] = warnings
    return summary, decoded


def har_summary(har, obs):
    if not isinstance(har, dict) or not isinstance(har.get("log", {}).get("entries"), list):
        raise AuditError("expected HAR log.entries")
    entries = har["log"]["entries"]
    put_count = matched = 0
    envelopes = collections.Counter()
    responses = collections.Counter()
    for entry in entries:
        req = entry.get("request", {})
        path = urlsplit(req.get("url", "")).path
        if req.get("method") == "PUT" and path.startswith("/run_data/"):
            put_count += 1
            body = parse_json(req.get("postData", {}).get("text", ""))
            matched += body == obs
        if urlsplit(req.get("url", "")).hostname != "run.gxapp.iydsj.com":
            continue
        content = entry.get("response", {}).get("content", {})
        try:
            response_text = content.get("text", "")
            if content.get("encoding") == "base64":
                response_text = base64.b64decode(response_text, validate=True).decode("utf-8")
            response = parse_json(response_text)
            if isinstance(response, dict) and {"r", "s", "v"} <= response.keys():
                signature_size = len(base64.b64decode(response["s"], validate=True))
                responses[(signature_size, str(response["v"]) == "101")] += 1
        except (ValueError, TypeError, UnicodeError):
            pass
        body = req.get("postData", {}).get("text")
        if not body:
            continue
        try:
            value = parse_json(body)
        except AuditError:
            continue
        if isinstance(value, dict) and {"d", "h", "k", "p", "t"} <= value.keys():
            # Only lengths are observed; no algorithm is inferred from them.
            sizes = []
            for key in ("k", "d"):
                try:
                    sizes.append(len(base64.b64decode(value[key], validate=True)))
                except (ValueError, TypeError):
                    sizes.append(-1)
            envelopes[(sizes[0], len(str(value["h"])), str(value["p"]) == "101")] += 1
    return {
        "entry_count": len(entries), "obs_upload_count": put_count,
        "matching_obs_upload_count": matched,
        "request_envelopes": [{"k_bytes": k, "h_chars": h, "p_is_101": p, "count": c}
                              for (k, h, p), c in sorted(envelopes.items())],
        "response_envelopes": [{"s_bytes": size, "v_is_101": v, "count": count}
                               for (size, v), count in sorted(responses.items())],
        "live_attribution": "unknown", "server_acceptance": "unknown",
    }


def normalization_check(original, normalized):
    a, b = copy.deepcopy(original), copy.deepcopy(normalized)
    for har in (a, b):
        for e in har.get("log", {}).get("entries", []):
            e.get("response", {}).get("content", {}).pop("mimeType", None)
    return {"identical": original == normalized, "equivalent_except_response_mime_type": a == b}


def audit(args):
    report = {"format": "run-capture-audit-v1", "privacy": "aggregate_allowlist_only", "sources": {}}
    inputs = {}
    for name in ("obs", "har", "normalized_har", "run_data", "fields", "candidate"):
        path = getattr(args, name, None)
        if path:
            inputs[name], digest = load_json(path)
            report["sources"][name] = {"sha256": digest}
    report["capture"], decoded = summarize_obs(inputs["obs"])
    if "har" in inputs:
        report["har"] = har_summary(inputs["har"], inputs["obs"])
    if "normalized_har" in inputs:
        report["normalized_har"] = har_summary(inputs["normalized_har"], inputs["obs"])
        if "har" in inputs:
            report["normalization"] = normalization_check(inputs["har"], inputs["normalized_har"])
    checks = {}
    if "run_data" in inputs:
        checks["run_data_matches_obs"] = inputs["run_data"] == _object(decoded["run_data"])
    if "fields" in inputs:
        fields = inputs["fields"]
        if not isinstance(fields, dict) or not fields:
            raise AuditError("expected decoded companion fields")
        checks["decoded_fields_match_obs"] = all(k in decoded and decoded[k] == v for k, v in fields.items())
    report["companion_checks"] = checks
    if "candidate" in inputs:
        report["candidate"], _ = summarize_obs(inputs["candidate"])
    report["integrity_ok"] = all(checks.values()) and all(
        report[k]["obs_upload_count"] > 0 and report[k]["matching_obs_upload_count"] == report[k]["obs_upload_count"]
        for k in ("har", "normalized_har") if k in report
    ) and report.get("normalization", {}).get("equivalent_except_response_mime_type", True)
    return report


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--obs", required=True, help="encoded COS/OBS JSON")
    for name in ("har", "normalized-har", "run-data", "fields", "candidate"):
        parser.add_argument("--" + name, help="optional local input")
    parser.add_argument("--out", help="aggregate JSON report; defaults to stdout")
    args = parser.parse_args(argv)
    try:
        if args.out:
            target = Path(args.out)
            for name in ("obs", "har", "normalized_har", "run_data", "fields", "candidate"):
                source = getattr(args, name)
                if source and (target.resolve() == Path(source).resolve()
                               or (target.exists() and target.samefile(source))):
                    raise AuditError("output must not overwrite an input")
        report = audit(args)
        output = json.dumps(report, ensure_ascii=False, indent=2, allow_nan=False) + "\n"
        if args.out:
            Path(args.out).write_text(output, encoding="utf-8")
        else:
            print(output, end="")
        return 0 if report["integrity_ok"] else 2
    except (AuditError, OSError, ValueError, TypeError, KeyError, OverflowError, AttributeError, RecursionError) as exc:
        # Exception messages from parsers/filesystems can contain private input.
        message = str(exc) if isinstance(exc, AuditError) else "invalid or unreadable local input"
        print("audit failed: " + message, file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
