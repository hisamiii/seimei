"""The cost scan keeps only what pricing reads (2026-10-21: whole usage records held 200 MB of 65k messages)."""
import json

from seimei import cost

FULL = {"input_tokens": 3, "output_tokens": 812, "cache_read_input_tokens": 51234, "cache_creation_input_tokens": 0,
        "cache_creation": {"ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": 4410},
        "server_tool_use": {"web_search_requests": 2, "web_fetch_requests": 0}, "service_tier": "standard",
        "speed": "fast", "inference_geo": "us", "iterations": [{"input_tokens": 3}]}


def test_slim_prices_the_same():
    for model in ("claude-opus-5-5", "claude-sonnet-4-5", "claude-haiku-5-5"):
        assert cost.cost(model, cost.slim(FULL)) == cost.cost(model, FULL)


def test_scan_skips_lines_without_usage(tmp_path):
    (tmp_path / "p").mkdir()
    rows = [{"type": "user", "message": {"content": "x" * 1000}, "sessionId": "s"},
            {"type": "assistant", "sessionId": "s", "timestamp": "2026-10-21T10:00:00Z", "cwd": "/w",
             "message": {"id": "m1", "model": "claude-opus-5-5", "usage": FULL}}]
    (tmp_path / "p" / "s.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
    c = cost.Cost(tmp_path)
    c.scan()
    assert list(c.messages) == ["m1"] and c.messages["m1"][2] == cost.slim(FULL)
