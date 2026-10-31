"""Synthetic-only regression tests; never load a real account or capture."""
import base64
import contextlib
import copy
import gzip
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import audit_run_capture as audit


def encoded(text):
    return base64.b64encode(gzip.compress(text.encode())).decode()


def sample():
    points = [{"gLat": 39.9, "gLng": 116.4, "totalTime": t, "totalDis": t,
               "gainTime": f"2026-01-01 10:00:{t:02d}", "coorType": "gcj02"}
              for t in (0, 10, 20)]
    speed = [{"beginTime": k * 10000, "endTime": (k + 1) * 10000, "distance": 10,
              "id": k + 1, "queueNum": 0} for k in (0, 1)]
    steps = [{**row, "stepsNum": 16} for row in speed]
    plain = {key: "" for key in audit.OBS_FIELDS}
    plain.update({
        "uid": "PRIVATE_USER", "uuid": "PRIVATE_UUID", "rrid": "PRIVATE_RECORD",
        "run_data": json.dumps({"allLocJson": json.dumps(points), "useZip": False}),
        "speed_json": json.dumps(speed), "step_freq_json": json.dumps(steps),
        "laps_json": json.dumps([{"distance": 20, "duration": 20, "step": 32}]),
        "fixed_point_json": json.dumps({"fivePointJson": "[]", "useZip": False}),
    })
    return {k: encoded(v) for k, v in plain.items()}, plain


class AuditTests(unittest.TestCase):
    def test_aggregates_never_include_identity_or_unknown_payload_fields(self):
        obj, _ = sample()
        obj["PRIVATE_UNKNOWN_FIELD"] = "PRIVATE_CONTENT"
        summary, _ = audit.summarize_obs(obj)
        self.assertEqual(summary["point_count"], 3)
        self.assertEqual(summary["track_time_span_s"], 20)
        self.assertEqual(summary["wall_time_range_s"], 20)
        self.assertEqual(summary["step_window_sum"], 32)
        self.assertEqual(summary["observations"], [])
        self.assertEqual(summary["unknown_field_count"], 1)
        self.assertNotIn("PRIVATE", json.dumps(summary))

    def test_pause_and_distance_step_disagreements_are_observations(self):
        obj, plain = sample()
        points = json.loads(json.loads(plain["run_data"])["allLocJson"])
        points[-1]["totalTime"] = 10
        obj["run_data"] = encoded(json.dumps({"allLocJson": json.dumps(points)}))
        obj["speed_json"] = encoded(json.dumps([
            {"distance": 3, "beginTime": 0, "endTime": 66005, "id": 1, "queueNum": 0}]))
        obj["step_freq_json"] = encoded(json.dumps([
            {"stepsNum": 20, "beginTime": 0, "endTime": 66005, "id": 1, "queueNum": 0}]))
        summary, _ = audit.summarize_obs(obj)
        self.assertEqual(set(summary["observations"]), {
            "wall_and_track_clocks_differ", "nonincreasing_track_time", "irregular_window_duration",
            "window_distance_differs_from_track", "lap_steps_differ_from_windows"})

    def test_missing_extension_is_reported(self):
        obj, _ = sample()
        del obj["extension_json"]
        summary, _ = audit.summarize_obs(obj)
        self.assertEqual(summary["missing_fields"], ["extension_json"])

    def test_duplicate_keys_and_nonfinite_numbers_are_rejected(self):
        for text in ('{"a":1,"a":2}', '{"a":{"b":1,"b":2}}', '{"a":NaN}', '{"a":1e999}'):
            with self.assertRaises(audit.AuditError):
                audit.parse_json(text)

    def test_bad_compression_and_expansion_are_rejected(self):
        with self.assertRaises(audit.AuditError):
            audit.decode_obs({"run_data": "not base64"})
        with mock.patch.object(audit, "MAX_FIELD", 64):
            with self.assertRaises(audit.AuditError):
                audit.decode_obs({"run_data": encoded("x" * 1000)})

    def test_har_matches_object_without_exporting_signed_url_or_token(self):
        obj, _ = sample()
        har = {"log": {"entries": [{"request": {
            "method": "PUT", "url": "https://example.invalid/run_data/private.json?q-signature=PRIVATE_SIGN",
            "headers": [{"name": "token", "value": "PRIVATE_TOKEN"}],
            "postData": {"text": json.dumps(obj)},
        }}]}}
        result = audit.har_summary(har, obj)
        self.assertEqual(result["matching_obs_upload_count"], 1)
        self.assertEqual(result["server_acceptance"], "unknown")
        self.assertNotIn("PRIVATE", json.dumps(result))
        self.assertEqual(audit.har_summary(har, {})["matching_obs_upload_count"], 0)

    def test_normalization_does_not_mask_non_mime_changes(self):
        a = {"log": {"entries": [{"response": {"content": {"mimeType": "x"}}}]}}
        b = copy.deepcopy(a)
        b["log"]["entries"][0]["response"]["content"]["mimeType"] = "y"
        self.assertTrue(audit.normalization_check(a, b)["equivalent_except_response_mime_type"])
        b["log"]["entries"][0]["request"] = {"method": "PUT"}
        self.assertFalse(audit.normalization_check(a, b)["equivalent_except_response_mime_type"])

    def test_bd_to_gcj_residual_is_not_a_raw_coordinate_distance(self):
        obj, _ = sample()
        gcj = (39.9, 116.4)
        # Approximate GCJ->BD transform independently from the audit's inverse.
        import math
        z = math.hypot(*gcj[::-1]) + .00002 * math.sin(gcj[0] * math.pi * 3000 / 180)
        theta = math.atan2(gcj[0], gcj[1]) + .000003 * math.cos(gcj[1] * math.pi * 3000 / 180)
        bd = (z * math.sin(theta) + .006, z * math.cos(theta) + .0065)
        obj["fixed_point_json"] = encoded(json.dumps({"fivePointJson": json.dumps([
            {"lat": bd[0], "lon": bd[1], "glat": gcj[0], "glon": gcj[1], "pointName": "PRIVATE_NAME"}])}))
        result, _ = audit.summarize_obs(obj)
        self.assertLess(result["checkpoint_bd09_to_gcj02_residual_m"][0], .2)
        self.assertEqual(result["checkpoint_nearest_recorded_point_m"], [0.0])
        self.assertNotIn("PRIVATE", json.dumps(result))

    def test_cli_bom_companions_mismatch_and_input_overwrite_guard(self):
        obj, _ = sample()
        with tempfile.TemporaryDirectory() as temp:
            path, fields, out = (Path(temp) / name for name in ("obs.json", "fields.json", "audit.json"))
            path.write_text(json.dumps(obj), encoding="utf-8-sig")
            fields.write_text(json.dumps({"extension_json": "wrong"}), encoding="utf-8")
            self.assertEqual(audit.main(["--obs", str(path), "--fields", str(fields), "--out", str(out)]), 2)
            result = json.loads(out.read_text(encoding="utf-8"))
            self.assertFalse(result["companion_checks"]["decoded_fields_match_obs"])
            self.assertNotIn("PRIVATE", out.read_text(encoding="utf-8"))
            before = path.read_bytes()
            with contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(audit.main(["--obs", str(path), "--out", str(path)]), 1)
            self.assertEqual(path.read_bytes(), before)

    def test_envelope_lengths_are_decoded_without_guessing_algorithms(self):
        obj, _ = sample()
        request = {"d": base64.b64encode(b"ciphertext").decode(), "h": "0" * 32,
                   "k": base64.b64encode(b"k" * 128).decode(), "p": "101", "t": "0"}
        response = {"r": "opaque", "s": base64.b64encode(b"s" * 128).decode(), "v": 101}
        har = {"log": {"entries": [{"request": {
            "url": "https://run.gxapp.iydsj.com/api/test", "method": "POST",
            "postData": {"text": json.dumps(request)},
        }, "response": {"content": {"text": json.dumps(response)}}}]}}
        result = audit.har_summary(har, obj)
        self.assertEqual(result["request_envelopes"], [{"k_bytes": 128, "h_chars": 32, "p_is_101": True, "count": 1}])
        self.assertEqual(result["response_envelopes"], [{"s_bytes": 128, "v_is_101": True, "count": 1}])


if __name__ == "__main__":
    unittest.main()
