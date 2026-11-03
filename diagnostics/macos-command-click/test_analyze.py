import io
import json
import unittest

from analyze import analyze, report


def event(kind, count, command=True, button=0):
    return json.dumps({"type": kind, "button": button, "clickCount": count, "command": command})


class AnalyzeTests(unittest.TestCase):
    def test_old_release_zero_fails_even_with_command_held(self):
        result = analyze([
            event("leftMouseDown", 1),
            json.dumps({"type": "flagsChanged", "command": True}),
            event("leftMouseUp", 0),
        ])
        self.assertTrue(result.failed)
        self.assertEqual(len(result.pairs), 1)
        self.assertTrue(result.pairs[0].down.command and result.pairs[0].up.command)
        output = io.StringIO()
        report(result, output)
        self.assertIn("down:L1/up:L3 clickCount=1/0 Command=true/true（保持） FAIL", output.getvalue())
        self.assertIn("clickCount < 1", output.getvalue())

    def test_matching_single_and_double_clicks_pair_by_button(self):
        # Interleaved buttons must pair independently, preserving click counts.
        result = analyze([
            event("leftMouseDown", 1),
            event("rightMouseDown", 2, command=False, button=1),
            event("leftMouseUp", 1),
            event("rightMouseUp", 2, command=False, button=1),
        ])
        self.assertFalse(result.failed)
        self.assertEqual([(p.down.button, p.down.count, p.up.count) for p in result.pairs],
                         [(0, 1, 1), (1, 2, 2)])
        self.assertEqual(result.issues, [])


if __name__ == "__main__":
    unittest.main()
