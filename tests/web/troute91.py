"""#91: where an event turn's closing reply goes. Unmarked: the chat. trajectory: noise stays in the trajectory, but a
trajectory: reply that asks something still reaches the chat."""
import time, requests
U = "http://localhost:8431"; O = {"Origin": U}
res = []
def check(n, ok, x=""): res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else x, flush=True)
s = requests.Session()
s.post(U + "/api/auth/password", json={"password": "qa-password-123"}, headers=O).raise_for_status()
def reply(cmd, words):
    """Sends !cmd and waits for the dispatcher's reply holding words."""
    s.post(U + "/api/send", json={"text": "!" + cmd}, headers=O).raise_for_status()
    end = time.time() + 30
    while time.time() < end:
        h = s.get(U + "/api/history?limit=40").json()["events"]
        got = [e for e in h if e["type"] == "assistant" and words in e.get("text", "")]
        if got:
            return got[-1]
        time.sleep(0.5)
    return {}
a = reply("eventplain", "plain cut")
check("an unmarked closing reply goes to the chat", a.get("lane") == "c" and a.get("from"), a)
a = reply("eventask", "Ship it?")
check("a trajectory: reply that asks goes to the chat anyway", a.get("lane") == "c" and a.get("from"), a)
a = reply("event", "all 12 frames")
check("trajectory: noise stays in the trajectory", a.get("lane") == "t", a)
print(f"{sum(res)}/{len(res)} passed")
