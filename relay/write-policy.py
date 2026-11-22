#!/usr/bin/env python3
"""Write policy for the peoples.chat relay: accept only Marmot messaging events."""
import json
import sys
import time
from collections import defaultdict, deque

ALLOWED_KINDS = {
    0,      # profile
    3,      # follow list: MDK publishes one with the profile when it sets an account up
    5,      # deletion request
    445,    # group message
    1059,   # gift wrap: invitations, device pairing, push requests
    10002,  # relay list
    10050,  # inbox relay list
    30443,  # key package
    30078,  # backup pointer
    443,    # older key package
    10051,  # older key package relay list
}
MAX_PER_MINUTE = 120  # events per network address
recent = defaultdict(deque)
handled = 0


def has_tag(event, name):
    return any(isinstance(t, list) and len(t) >= 2 and t[0] == name for t in event.get("tags", []))


def decide(req):
    event = req["event"]
    kind = event.get("kind")
    if kind not in ALLOWED_KINDS:
        return "reject", "blocked: this relay only carries Marmot messaging events"
    if kind == 445 and not has_tag(event, "h"):
        return "reject", "invalid: group message without an h tag"
    if kind == 1059 and not has_tag(event, "p"):
        return "reject", "invalid: gift wrap without a p tag"
    if req.get("sourceType") in ("IP4", "IP6"):
        now = time.time()
        window = recent[req.get("sourceInfo", "")]
        while window and now - window[0] > 60:
            window.popleft()
        if len(window) >= MAX_PER_MINUTE:
            return "reject", "rate-limited: slow down"
        window.append(now)
    return "accept", ""


for line in sys.stdin:
    try:
        req = json.loads(line)
        action, msg = decide(req)
        out = {"id": req["event"]["id"], "action": action}
        if msg:
            out["msg"] = msg
    except Exception as exc:  # report and keep running
        print(f"write-policy error: {exc}", file=sys.stderr, flush=True)
        continue
    print(json.dumps(out), flush=True)
    handled += 1
    if handled % 10000 == 0:  # forget idle addresses
        for key in [k for k, w in recent.items() if not w or time.time() - w[-1] > 60]:
            del recent[key]
