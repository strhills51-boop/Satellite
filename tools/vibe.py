#!/usr/bin/env python3
"""VIBE-GO semantic core, ported from the runner (vibe-go runner v6.7.0-cc-bootstrap…, src: VibeGoRunner).

A mission ledger kept as one JSON file (a SemanticSession), changed only through `apply`: the model proposes a
move, this reducer decides. What is ported, by the runner's names:

- SemanticSession / createSemanticSession: mission (direction, reminder), route (activeDistanceId, activity,
  deepeningStack, returnPoint, tacticalFocus), distances, notes (promises), decisions, worklog, strategicFrontier,
  revision, acceptanceSequence, hold.
- SEMANTIC_ACTIONS: CONTINUE, DEEPEN, RETURN, REDIRECT, DISTANCE_SEALED, NOTE_ADDED, WAIT_OPERATOR, HOLD,
  MISSION_SEALED (WAIT_TRANSPORT is DOM/relay transport — not ported).
- SEMANTIC_ACTIVITIES: RECON, ANALYSIS, CHANGE, VALIDATION, PACKAGING, CLOSURE (descriptive, not a phase gate).
- Distance statuses (SEMANTIC_DISTANCE_SCHEDULER_STATES_V677): active, ready, waiting_on_promise,
  waiting_on_human, resumable, suspended, sealed (waiting_on_transport — not ported).
- applySemanticState: the transitions; readyDistances (queueSemanticReadyDistancesV677), resumeDistances
  (applySemanticResumeDistancesV691), notes/promises, decisions, strategicFrontier patch, tacticalFocus.
- Laws, with the runner's error codes:
  ACTIVE_DISTANCE_BLOCKER_REACTIVATION_FORBIDDEN (a09.17 / FM-16),
  OPEN_MISSION_CONTINUITY_REQUIRES_SUCCESSOR and OPEN_MISSION_CONTINUITY_RETURN_REQUIRES_SUCCESSOR (P6.6 / V704),
  baseRevision mismatch (CAS), anti-preemption P14.G06 (scheduleNextSemanticDistanceV677),
  a45: the semantic revision moves only when meaning changed (semanticMeaningProjectionV644).
- Scheduler (selectSemanticSchedulerCandidateV677): resumable before ready, dependencyBlocking first, priority
  BLOCKING / HIGH / NORMAL / LOW, then age; with no candidate an open Mission gets a system continuity Distance
  (ensureOpenMissionContinuityDistanceV704).
- Human requests (CREATE_HUMAN_REQUEST, markSemanticDistanceHumanBlockedV677, reconcileSemanticDistanceSchedulerV677):
  a BLOCKING request parks its Distance as waiting_on_human; the answer makes it resumable; the answer is applied
  as HUMAN_DECISION and then consumed. DEFERRED requests do not block.
- Strategic Frontier (applyStrategicFrontierPatchV690): posture EXPLORE / CONSOLIDATE / EXECUTE / VALIDATE /
  ARCHITECT, activeTheory, strategyReminder, fronts with LOW / MEDIUM / HIGH informationGain, leverage,
  mechanicality, blockingValue, cost; tactical focus is advisory.
- Anti-tunnel (detectSemanticAntiTunnelV690), in its mission-neutral signals only: strategic review debt
  (≥ 5 Distances sealed since the last review) and work without effect (≥ 8 recent accepted moves with no
  artifact); a STRATEGY CHECK appears when such a pattern is active and ≥ 3 moves passed since the last review.
- Dispatch (buildSemanticDispatch, full protocol): the text the model reads before each move.

Not in the runner — added here, marked `[claude]` where enforced:
- CLOSURE_EVIDENCE_REQUIRED: DISTANCE_SEALED / RETURN seal / MISSION_SEALED carry `evidence` (a commit, a test,
  a CI run, an owner answer); a git hash is checked to exist. The runner's closureContract was text nobody checked.
- MISSION_SEALED_WITH_OPEN_WORK: a Mission does not seal while Distances are active / ready / resumable / waiting.

Usage:
  vibe.py init <ledger.json> --direction TEXT [--reminder TEXT] [--objective TEXT] [--closure TEXT]
  vibe.py dispatch <ledger.json>            # what to read before a move
  vibe.py apply <ledger.json> <payload.json|->  # one move (VG_RESULT-like JSON); rejected moves change nothing
  vibe.py answer <ledger.json> <request_id> <answer text>   # the owner's answer to a human request
"""

from __future__ import annotations

import json
import os
import random
import subprocess
import sys
import time
from datetime import datetime, timezone

SCHEMA = "vibe-go-semantic-session/v1"
ACTIONS = ["CONTINUE", "DEEPEN", "RETURN", "REDIRECT", "DISTANCE_SEALED", "NOTE_ADDED", "WAIT_OPERATOR", "HOLD",
           "MISSION_SEALED"]
ACTIVITIES = ["RECON", "ANALYSIS", "CHANGE", "VALIDATION", "PACKAGING", "CLOSURE"]
STATES = ["active", "ready", "waiting_on_promise", "waiting_on_human", "resumable", "suspended", "sealed"]
PRIORITY = {"BLOCKING": 0, "HIGH": 1, "NORMAL": 2, "LOW": 3}
POSTURES = ["EXPLORE", "CONSOLIDATE", "EXECUTE", "VALIDATE", "ARCHITECT"]
CONTINUITY = "SYSTEM_OPEN_MISSION_CONTINUITY"
MAX_FRONTS = 12


class Law(Exception):
    """A move the reducer refuses; nothing is written."""


def now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def sid(prefix: str) -> str:
    return f"{prefix}:{int(time.time() * 1000):x}:{random.randrange(16**8):08x}"


def text(v, n=1800) -> str:
    s = " ".join(str(v or "").split())
    return s[:n]


def priority(v, fallback="NORMAL") -> str:
    v = str(v or fallback).strip().upper()
    return v if v in PRIORITY else "NORMAL"


def activity(v, fallback="RECON") -> str:
    v = str(v or fallback).strip().upper()
    return v if v in ACTIVITIES else fallback


def distance(inp: dict, parent=None) -> dict:
    t = now()
    return {
        "id": str(inp.get("id") or sid("deepening" if parent else "distance")),
        "parentId": parent or inp.get("parentId"),
        "objective": text(inp.get("objective")) or "Продолжать активное направление до фактического результата.",
        "closureContract": text(inp.get("closureContract")),
        "status": str(inp.get("status") or "active"),
        "reason": text(inp.get("reason")),
        "summary": text(inp.get("summary")),
        "createdAt": t, "updatedAt": t, "sealedAt": None, "suspendedAt": None, "readyAt": None,
        "resumableAt": None, "resumedAt": None,
        "resumePriority": priority(inp.get("resumePriority") or inp.get("priority")),
        "dependencyBlocking": bool(inp.get("dependencyBlocking")),
        "strategicFrontId": inp.get("strategicFrontId"),
        "resumeContract": text(inp.get("resumeContract"), 900),
        "waitingOn": None, "schedulerReturn": None, "evidence": None,
    }


def front(inp: dict, i=0) -> dict:
    def coarse(v):
        v = str(v or "").strip().upper()
        return v if v in ("LOW", "MEDIUM", "HIGH") else None
    fid = str(inp.get("frontId") or inp.get("id") or f"front-{i + 1}")
    return {
        "frontId": fid, "title": text(inp.get("title") or fid, 220), "objective": text(inp.get("objective"), 700),
        "status": str(inp.get("status") or "OPEN").upper(),
        "informationGain": coarse(inp.get("informationGain")), "leverage": coarse(inp.get("leverage")),
        "mechanicality": coarse(inp.get("mechanicality")), "blockingValue": coarse(inp.get("blockingValue")),
        "cost": coarse(inp.get("cost")),
        "relatedDistanceIds": list(dict.fromkeys(inp.get("relatedDistanceIds") or []))[:12],
        "rationale": text(inp.get("rationale"), 700), "provenance": str(inp.get("provenance") or "AI_ASSESSMENT"),
    }


def session_new(direction, reminder, objective, closure) -> dict:
    root = distance({"objective": objective, "closureContract": closure})
    t = now()
    return {
        "schema": SCHEMA, "id": sid("semantic-session"), "revision": 0, "status": "active",
        "createdAt": t, "updatedAt": t,
        "mission": {"direction": direction, "reminder": reminder},
        "route": {"activeDistanceId": root["id"], "activity": "RECON", "deepeningStack": [], "returnPoint": None,
                  "tacticalFocus": "", "scheduler": {"events": []}},
        "distances": {root["id"]: root},
        "notes": [], "decisions": [], "worklog": [],
        "humanRequests": {},
        "strategicFrontier": {
            "activeTheory": "", "posture": "EXPLORE",
            "strategyReminder": "Tactical focus is advisory. Optimize Mission progress; prefer batch/tooling or "
                                "abstraction shifts when they have higher leverage.",
            "fronts": [], "reviewRevision": 0, "reviewAcceptanceSequence": 0, "reviewedAt": None,
        },
        "acceptanceSequence": 0, "lastControl": None, "hold": None, "completedAt": None,
    }


# ---------------------------------------------------------------- scheduler (V677 / V704)

def active(s):
    a = s["route"].get("activeDistanceId")
    return s["distances"].get(a) if a else None


def event(s, kind, **kw):
    ev = {"type": kind, "at": now(), **kw}
    s["route"]["scheduler"]["events"] = (s["route"]["scheduler"]["events"] + [ev])[-96:]


def rank(d):
    state = 0 if d["status"] == "resumable" else 1 if d["status"] == "ready" else 9
    return (state, 0 if d.get("dependencyBlocking") else 1, PRIORITY[priority(d.get("resumePriority"))],
            d.get("resumableAt") or d.get("readyAt") or d.get("createdAt") or "", d["id"])


def candidate(s):
    c = [d for d in s["distances"].values() if d["status"] in ("ready", "resumable")]
    return sorted(c, key=rank)[0] if c else None


def capture_return(s, d):
    if d.get("schedulerReturn"):
        return d["schedulerReturn"]
    d["schedulerReturn"] = {
        "capturedAt": now(), "deepeningStack": json.loads(json.dumps(s["route"]["deepeningStack"])),
        "returnPoint": s["route"].get("returnPoint"), "tacticalFocus": s["route"].get("tacticalFocus", ""),
        "resumeContract": d.get("resumeContract", ""), "objective": d["objective"],
        "closureContract": d.get("closureContract", ""),
    }
    return d["schedulerReturn"]


def continuity(s, reason):
    """ensureOpenMissionContinuityDistanceV704: an open Mission with nothing to do gets a continuity Distance."""
    if s["status"] != "active":
        return None
    a = active(s)
    if a and a["status"] == "active":
        return a
    c = candidate(s)
    if c:
        return c
    blocked = [d for d in s["distances"].values()
               if d.get("reason") == CONTINUITY and d["status"].startswith("waiting_on_")]
    if blocked:
        return None
    for d in s["distances"].values():
        if d.get("reason") == CONTINUITY and d["status"] in ("ready", "resumable", "active"):
            return d
    d = distance({
        "id": sid("mission-continuity"),
        "objective": "Reconcile the still-open Mission after local Distance completion. Use durable Mission "
                     "authority, accepted evidence, scheduler blockers and Strategic Frontier to activate, resume or "
                     "create the next highest-leverage executable Distance. Do not redo sealed work and do not treat "
                     "advisory focus as work authority.",
        "closureContract": "Leave this continuity Distance only by establishing successor executable Distance "
                           "authority, sealing the Mission, or entering an explicit genuine external-prerequisite "
                           "wait. It may not be sealed into an open Mission with no successor.",
        "status": "ready", "reason": CONTINUITY, "resumePriority": "BLOCKING", "dependencyBlocking": True,
    })
    d["readyAt"] = now()
    s["distances"][d["id"]] = d
    event(s, "OPEN_MISSION_CONTINUITY_QUEUED", distanceId=d["id"], reason=reason)
    return d


def activate(s, d, reason):
    d["status"] = "active"
    d["resumedAt"] = d["updatedAt"] = now()
    prev = s["route"].get("activeDistanceId")
    s["route"]["activeDistanceId"] = d["id"]
    r = d.get("schedulerReturn")
    if r:
        s["route"]["deepeningStack"] = r.get("deepeningStack") or []
        s["route"]["returnPoint"] = r.get("returnPoint")
        if r.get("tacticalFocus"):
            s["route"]["tacticalFocus"] = r["tacticalFocus"]
    else:
        s["route"]["deepeningStack"] = []
        s["route"]["returnPoint"] = None
    event(s, "ACTIVATED", distanceId=d["id"], previousDistanceId=prev, reason=reason)
    return d


def schedule(s, reason):
    """scheduleNextSemanticDistanceV677 — P14.G06: a coherent ACTIVE Distance is never preempted."""
    a = active(s)
    if a and a["status"] == "active":
        return a
    c = candidate(s) or continuity(s, reason)
    if not c:
        s["route"]["activeDistanceId"] = None
        s["route"]["deepeningStack"] = []
        s["route"]["returnPoint"] = None
        event(s, "IDLE_NO_READY", reason=reason)
        return None
    return activate(s, c, reason)


def human_block(s, d, req):
    """markSemanticDistanceHumanBlockedV677: only a BLOCKING request parks its Distance."""
    if str(req.get("blocking_class") or "DEFERRED").upper() != "BLOCKING":
        return False
    if s["route"].get("activeDistanceId") == d["id"]:
        capture_return(s, d)
    w = d.get("waitingOn") or {"humanRequestIds": [], "humanStates": {}}
    if req["request_id"] not in w["humanRequestIds"]:
        w["humanRequestIds"].append(req["request_id"])
    w["humanStates"][req["request_id"]] = "OPEN"
    w["reason"] = text(req.get("question") or "human decision required")
    w["resumeContract"] = text(req.get("resume_contract") or "apply HUMAN_DECISION authority and resume this Distance")
    d["waitingOn"] = w
    d["resumePriority"] = priority(req.get("resume_priority"), "BLOCKING")
    d["status"] = "waiting_on_human"
    d["updatedAt"] = now()
    if s["route"].get("activeDistanceId") == d["id"]:
        s["route"]["activeDistanceId"] = None
    event(s, "WAITING_ON_HUMAN", distanceId=d["id"], requestId=req["request_id"], reason=w["reason"])
    return True


def reconcile(s, reason="inbox-reconcile"):
    """reconcileSemanticDistanceSchedulerV677, human side: answered requests make their Distance resumable."""
    for d in s["distances"].values():
        w = d.get("waitingOn")
        if not w or d["status"] != "waiting_on_human":
            continue
        open_ = False
        for rid in w.get("humanRequestIds", []):
            st = s["humanRequests"].get(rid, {}).get("status", "OPEN")
            w["humanStates"][rid] = st
            if st not in ("ANSWERED", "CONSUMED", "CANCELLED"):
                open_ = True
        if not open_:
            d["status"] = "resumable"
            d["resumableAt"] = d["updatedAt"] = now()
            event(s, "RESUMABLE", distanceId=d["id"], reason=w.get("resumeContract") or reason)
    a = active(s)
    if not a or a["status"] != "active":
        schedule(s, reason)


# ---------------------------------------------------------------- strategic frontier and anti-tunnel (V690)

def frontier_patch(s, patch, revision) -> bool:
    f = s["strategicFrontier"]
    changed = False
    for k, n in (("activeTheory", 1200), ("strategyReminder", 900)):
        if k in patch and text(patch[k], n) != f[k]:
            f[k] = text(patch[k], n)
            changed = True
    if "posture" in patch:
        p = str(patch["posture"]).upper()
        if p in POSTURES and p != f["posture"]:
            f["posture"] = p
            changed = True
    if isinstance(patch.get("upsertFronts"), list):
        by = {x["frontId"]: x for x in f["fronts"]}
        for i, raw in enumerate(patch["upsertFronts"]):
            nf = front(raw, i)
            if by.get(nf["frontId"]) != nf:
                by[nf["frontId"]] = nf
                changed = True
        f["fronts"] = list(by.values())[-MAX_FRONTS:]
    if isinstance(patch.get("closeFrontIds"), list):
        for x in f["fronts"]:
            if x["frontId"] in patch["closeFrontIds"] and x["status"] != "CLOSED":
                x["status"] = "CLOSED"
                changed = True
    if changed or patch.get("reviewed") is True:
        f["reviewRevision"] = revision
        f["reviewAcceptanceSequence"] = s["acceptanceSequence"]
        f["reviewedAt"] = now()
    return changed or patch.get("reviewed") is True


def anti_tunnel(s) -> dict:
    rows = s["worklog"][-8:]
    reasons = []
    reviewed = s["strategicFrontier"].get("reviewedAt") or ""
    sealed_since = sum(1 for d in s["distances"].values() if d["status"] == "sealed" and (d.get("sealedAt") or "") > reviewed)
    if sealed_since >= 5:
        reasons.append(f"strategic review debt ({sealed_since} Distances sealed since the last recorded review)")
    idle = [r for r in rows if r.get("artifactEffect", "NONE") == "NONE"]
    if len(rows) >= 8 and len(idle) == len(rows):
        reasons.append(f"work without effect ({len(rows)} recent accepted moves with no artifact/commit)")
    since = s["acceptanceSequence"] - s["strategicFrontier"].get("reviewAcceptanceSequence", 0)
    return {"active": bool(reasons), "reviewDue": bool(reasons) and since >= 3, "reasons": reasons}


# ---------------------------------------------------------------- the reducer (applySemanticState)

def meaning(s) -> str:
    """semanticMeaningProjectionV644: what counts as meaning (activity / focus are control only)."""
    keep = ("id", "parentId", "objective", "closureContract", "reason", "status", "summary", "resumePriority",
            "dependencyBlocking", "waitingOn", "evidence")
    return json.dumps({
        "status": s["status"], "mission": s["mission"],
        "distances": {k: {f: d.get(f) for f in keep} for k, d in sorted(s["distances"].items())},
        "notes": [(n["kind"], n["text"], n["status"]) for n in s["notes"]],
        "decisions": [d["text"] for d in s["decisions"]],
        "humans": {k: v.get("status") for k, v in s["humanRequests"].items()},
        "frontier": {k: s["strategicFrontier"][k] for k in ("activeTheory", "posture", "fronts")},
    }, sort_keys=True, ensure_ascii=False)


def check_evidence(ev) -> dict:
    """[claude] CLOSURE_EVIDENCE_REQUIRED: a seal names what proves it; a git hash must exist."""
    if not ev:
        raise Law("CLOSURE_EVIDENCE_REQUIRED [claude]: a seal needs `evidence` — a commit, a test, a CI run or an "
                  "owner answer; the closureContract alone is what the model says, not what is true.")
    items = ev if isinstance(ev, list) else [ev]
    out = []
    for e in items:
        e = {"text": e} if isinstance(e, str) else dict(e)
        c = e.get("commit")
        if c:
            ok = subprocess.run(["git", "cat-file", "-e", f"{c}^{{commit}}"], capture_output=True).returncode == 0
            if not ok:
                raise Law(f"CLOSURE_EVIDENCE_UNKNOWN_COMMIT [claude]: {c} is not a commit of this repository")
        out.append(e)
    return {"items": out, "at": now()}


def note(s, v, revision):
    v = {"text": v} if isinstance(v, str) else v
    t = text(v.get("text"))
    if not t:
        return
    nid = str(v.get("id") or sid("note"))
    for n in s["notes"]:
        if n["id"] == nid or (n["kind"] == str(v.get("kind") or "finding") and n["text"] == t):
            n.update(status=str(v.get("status") or n["status"]), revision=revision, updatedAt=now())
            return
    s["notes"].append({"id": nid, "kind": str(v.get("kind") or "finding"), "text": t,
                       "status": str(v.get("status") or "open"), "distanceId": s["route"].get("activeDistanceId"),
                       "meta": v.get("meta"), "revision": revision, "createdAt": now(), "updatedAt": now()})


def decision(s, v, revision):
    v = {"text": v} if isinstance(v, str) else v
    t = text(v.get("text"))
    if t and not any(d["text"] == t for d in s["decisions"]):
        s["decisions"].append({"id": str(v.get("id") or sid("decision")), "text": t,
                               "reason": text(v.get("reason"), 900), "distanceId": s["route"].get("activeDistanceId"),
                               "revision": revision, "createdAt": now()})


def upsert(s, inp, parent=None):
    i = inp.get("id")
    if i and i in s["distances"]:
        d = s["distances"][i]
        for k in ("objective", "closureContract", "reason", "summary", "resumeContract", "strategicFrontId"):
            if inp.get(k):
                d[k] = text(inp[k])
        d["updatedAt"] = now()
        return d
    d = distance(inp, parent)
    s["distances"][d["id"]] = d
    return d


def apply(s0: dict, p: dict) -> dict:
    s = json.loads(json.dumps(s0))
    action = str(p.get("action") or "CONTINUE").upper().removeprefix("VIBE_")
    if action not in ACTIONS:
        raise Law(f"Unknown VIBE_STATE action: {action}")
    if s["status"] != "active" and action not in ("CONTINUE", "NOTE_ADDED"):
        if not (s["status"] == "hold" and p.get("release")):
            raise Law(f"SESSION_NOT_ACTIVE: session is {s['status']}")
    if s["status"] == "hold" and p.get("release"):
        s["status"], s["hold"] = "active", None
    if "baseRevision" in p and int(p["baseRevision"]) != s["revision"]:
        raise Law(f"VIBE_STATE baseRevision mismatch: expected {s['revision']}")
    prev_rev = s["revision"]
    rev = prev_rev + 1
    before = meaning(s)
    t = now()

    if action != "WAIT_OPERATOR":
        if p.get("direction"):
            s["mission"]["direction"] = str(p["direction"])
        if p.get("reminder"):
            s["mission"]["reminder"] = str(p["reminder"])
        if p.get("activity"):
            s["route"]["activity"] = activity(p["activity"], s["route"]["activity"])
        if "tacticalFocus" in p or "nextFocus" in p:
            s["route"]["tacticalFocus"] = text(p.get("tacticalFocus", p.get("nextFocus")), 1200)
        if isinstance(p.get("strategicFrontier"), dict):
            frontier_patch(s, p["strategicFrontier"], rev)
        for n in (p.get("notes") or []) + (p.get("promises") or []):
            note(s, n, rev)
        for d in p.get("decisions") or []:
            decision(s, d, rev)
        for inp in p.get("readyDistances") or []:
            d = upsert(s, inp, inp.get("parentId"))
            if d["id"] != s["route"].get("activeDistanceId") and d["status"] != "sealed":
                d["status"] = "ready"
                d["resumePriority"] = priority(inp.get("resumePriority") or d["resumePriority"])
                d["dependencyBlocking"] = bool(inp.get("dependencyBlocking", d["dependencyBlocking"]))
                d["readyAt"] = d["readyAt"] or t
                event(s, "READY_QUEUED", distanceId=d["id"])
        for inp in p.get("resumeDistances") or []:
            d = s["distances"].get(str(inp.get("id") or ""))
            if not d or d["status"] == "sealed":
                continue
            capture_return(s, d)
            if inp.get("tacticalFocus"):
                d["schedulerReturn"]["tacticalFocus"] = text(inp["tacticalFocus"], 1200)
            if inp.get("resumeContract"):
                d["resumeContract"] = d["schedulerReturn"]["resumeContract"] = text(inp["resumeContract"], 900)
            d["status"] = "resumable" if str(inp.get("status")).lower() == "resumable" else "ready"
            d["resumePriority"] = priority(inp.get("resumePriority") or d["resumePriority"])
            d["readyAt" if d["status"] == "ready" else "resumableAt"] = t
            event(s, "STRATEGIC_RESUME_QUEUED", distanceId=d["id"])
        for raw in p.get("humanRequests") or []:
            req = dict(raw)
            req["request_id"] = str(req.get("request_id") or sid("HRQ"))
            req.setdefault("status", "OPEN")
            req.setdefault("blocking_class", "DEFERRED")
            req["createdAt"] = t
            s["humanRequests"][req["request_id"]] = req
            target = s["distances"].get(str(req.get("distance_id") or s["route"].get("activeDistanceId") or ""))
            if target:
                req["distance_id"] = target["id"]
                human_block(s, target, req)
        for rid in p.get("consumeHumanRequests") or []:
            r = s["humanRequests"].get(rid)
            if not r or r.get("status") != "ANSWERED":
                raise Law(f"HUMAN_REQUEST_NOT_ANSWERED: {rid} — consume only an answered request, after applying its "
                          "answer as HUMAN_DECISION")
            r["status"] = "CONSUMED"

        before_active = active(s)
        ad = p.get("activeDistance")
        if ad and action != "DEEPEN":
            ex = s["distances"].get(str(ad.get("id") or ""))
            ex_st = (ex or {}).get("status", "")
            req_st = str(ad.get("status") or "").lower()
            if ex_st.startswith("waiting_on_") or ex_st == "sealed" or req_st.startswith("waiting_on_") or req_st == "sealed":
                raise Law("ACTIVE_DISTANCE_BLOCKER_REACTIVATION_FORBIDDEN: activeDistance cannot reactivate or "
                          f"manufacture a durable waiting/sealed Distance. distance={ad.get('id') or '(new)'} "
                          f"existingStatus={ex_st or '(none)'}. Keep the blocker intact; seal/continue the current "
                          "branch and let the scheduler resume blocked work only after the blocker resolves.")
            d = upsert(s, ad, ad.get("parentId") or (before_active or {}).get("parentId"))
            d["status"] = "active"
            s["route"]["activeDistanceId"] = d["id"]

    cur = active(s)
    if action == "WAIT_OPERATOR":
        s["operatorPrerequisite"] = {"status": "waiting", "text": text(p.get("reason") or p.get("summary")), "at": t}
    elif action in ("CONTINUE", "NOTE_ADDED"):
        pass
    elif action == "DEEPEN":
        if cur:
            cur["status"], cur["suspendedAt"], cur["updatedAt"] = "suspended", t, t
        inp = p.get("activeDistance") or p.get("distance") or {
            "objective": p.get("objective"), "closureContract": p.get("closureContract"), "reason": p.get("reason")}
        child = upsert(s, inp, cur["id"] if cur else None)
        child["status"] = "active"
        rp = p.get("returnPoint") or {"tacticalFocus": s["route"].get("tacticalFocus", ""),
                                      "posture": s["strategicFrontier"]["posture"]}
        s["route"]["deepeningStack"].append({"distanceId": cur["id"] if cur else None, "childDistanceId": child["id"],
                                             "returnPoint": rp, "openedAt": t})
        s["route"]["returnPoint"] = rp
        s["route"]["activeDistanceId"] = child["id"]
    elif action in ("RETURN", "DISTANCE_SEALED"):
        sealing = action == "DISTANCE_SEALED" or p.get("seal") is True
        if cur and cur.get("reason") == CONTINUITY and not s["route"]["deepeningStack"] and not candidate(s) \
                and (action == "RETURN" or not p.get("nextDistance")) and sealing:
            code = "OPEN_MISSION_CONTINUITY_RETURN_REQUIRES_SUCCESSOR" if action == "RETURN" else \
                "OPEN_MISSION_CONTINUITY_REQUIRES_SUCCESSOR"
            raise Law(f"{code}: a system continuity Distance cannot be sealed while the Mission remains open without "
                      "nextDistance/READY/RESUMABLE work. Use REDIRECT/nextDistance, establish a durable blocker, "
                      "WAIT_OPERATOR for a genuine external prerequisite, or MISSION_SEALED if the Mission is complete.")
        if cur:
            if sealing and cur.get("reason") != CONTINUITY:
                cur["evidence"] = check_evidence(p.get("evidence"))
            cur["status"] = "sealed" if sealing else "suspended"
            cur["summary"] = text(p.get("summary") or cur["summary"])
            cur["updatedAt"] = t
            cur["sealedAt" if sealing else "suspendedAt"] = t
        frame = s["route"]["deepeningStack"].pop() if s["route"]["deepeningStack"] else None
        parent = s["distances"].get(frame["distanceId"]) if frame and frame.get("distanceId") else None
        if parent:
            parent["status"], parent["updatedAt"] = "active", t
            s["route"]["activeDistanceId"] = parent["id"]
            if (frame.get("returnPoint") or {}).get("tacticalFocus"):
                s["route"]["tacticalFocus"] = frame["returnPoint"]["tacticalFocus"]
        elif action == "DISTANCE_SEALED" and p.get("nextDistance"):
            nxt = upsert(s, p["nextDistance"], None)
            nxt["status"] = "active"
            s["route"]["activeDistanceId"] = nxt["id"]
        else:
            s["route"]["activeDistanceId"] = None
            schedule(s, "distance-sealed")
        s["route"]["returnPoint"] = (s["route"]["deepeningStack"][-1]["returnPoint"]
                                     if s["route"]["deepeningStack"] else None)
        if p.get("promise"):
            note(s, p["promise"], rev)
    elif action == "REDIRECT":
        nxt = p.get("nextDistance") or p.get("activeDistance")
        if nxt:
            if cur:
                capture_return(s, cur)
                if p.get("resumeContract"):
                    cur["resumeContract"] = cur["schedulerReturn"]["resumeContract"] = text(p["resumeContract"], 900)
                cur["status"], cur["suspendedAt"], cur["updatedAt"] = "suspended", t, t
            red = upsert(s, nxt, nxt.get("parentId", (cur or {}).get("parentId")))
            red["status"] = "active"
            s["route"]["activeDistanceId"] = red["id"]
            event(s, "STRATEGIC_REDIRECT", previousDistanceId=(cur or {}).get("id"), distanceId=red["id"],
                  reason=text(p.get("reason") or "model strategic redirect"))
        elif cur:
            if p.get("objective"):
                cur["objective"] = text(p["objective"])
            if p.get("closureContract"):
                cur["closureContract"] = text(p["closureContract"])
            cur["reason"] = text(p.get("reason") or cur["reason"])
            cur["updatedAt"] = t
    elif action == "HOLD":
        s["status"] = "hold"
        s["hold"] = {"reason": text(p.get("reason") or p.get("summary") or "Semantic HOLD"), "at": t, "revision": rev}
    elif action == "MISSION_SEALED":
        open_ = [d["id"] for d in s["distances"].values()
                 if d["status"] in ("active", "ready", "resumable") or d["status"].startswith("waiting_on_")]
        open_ = [i for i in open_ if s["distances"][i].get("reason") != CONTINUITY]
        if open_:
            raise Law(f"MISSION_SEALED_WITH_OPEN_WORK [claude]: open Distances {open_} — seal, supersede or return them")
        s["missionEvidence"] = check_evidence(p.get("evidence"))
        for d in s["distances"].values():
            if d.get("reason") == CONTINUITY and d["status"] != "sealed":
                d["status"], d["sealedAt"] = "sealed", t
        s["status"], s["completedAt"] = "sealed", t
        s["route"]["activeDistanceId"] = None

    if action != "WAIT_OPERATOR" and s.get("operatorPrerequisite", {}).get("status") == "waiting":
        s["operatorPrerequisite"]["status"] = "cleared"
    a = active(s)
    if s["status"] == "active" and (not a or a["status"] != "active"):
        schedule(s, "post-apply")

    changed = meaning(s) != before or bool(p.get("artifacts"))
    s["acceptanceSequence"] += 1
    if changed:
        s["revision"] = rev
        s["worklog"].append({
            "id": sid("worklog"), "revision": rev, "at": t, "action": action, "activity": s["route"]["activity"],
            "artifactEffect": "ARTIFACT" if p.get("artifacts") else "NONE", "artifacts": p.get("artifacts") or [],
            "distanceId": s["route"].get("activeDistanceId"), "summary": text(p.get("summary"), 1600),
        })
    s["lastControl"] = {"at": t, "revision": s["revision"], "semanticChanged": changed,
                        "acceptanceSequence": s["acceptanceSequence"], "modelSummary": text(p.get("summary"), 1600)}
    s["updatedAt"] = t
    return s


# ---------------------------------------------------------------- dispatch (buildSemanticDispatch, full protocol)

def stack_text(s) -> str:
    ids = [f["distanceId"] for f in s["route"]["deepeningStack"] if f.get("distanceId")]
    a = s["route"].get("activeDistanceId")
    if a:
        ids.append(a)
    ids = list(dict.fromkeys(ids))
    if not ids:
        return "(no active Distance)"
    return "\n".join(f"{'  ' * i}- {d} [{s['distances'][d]['status']}] {s['distances'][d]['objective']}"
                     for i, d in enumerate(ids))


def dispatch(s) -> str:
    a = active(s)
    by = lambda st: sorted([d for d in s["distances"].values() if d["status"] == st], key=rank)  # noqa: E731
    f = s["strategicFrontier"]
    at = anti_tunnel(s)
    out = [
        f"[[VIBE_DISPATCH session={s['id']} revision={s['revision']} status={s['status']}]]",
        "",
        "MODEL-OPERATING SEMANTICS · RUNTIME CONTRACT",
        "Ledger keeps semantic continuity, causal position and durable obligations; the Mission comes only from this "
        "session authority.",
        "Distance — a real semantic/engineering obligation up to an actual result, not an answer, checkpoint or chunk.",
        "A new Distance is needed when the actual obligation changes; a new independent branch by semantic necessity.",
        "Activity describes the dominant kind of current work and is not a phase sequence.",
        "The model owns engineering reasoning; the reducer owns continuity/causality/durability and refuses illegal moves.",
        "",
        "DIRECTION", s["mission"]["direction"], "",
        "REMINDER", s["mission"]["reminder"] or "(none)", "",
        "CURRENT ACTIVITY · DESCRIPTIVE, NOT A PHASE GATE", s["route"]["activity"], "",
        "DISTANCE STACK · MEMORY, NOT A WORKFLOW", stack_text(s), "",
        f"DISTANCE SCHEDULER · active {a['id'] if a else 'none'} · ready {len(by('ready'))} · resumable "
        f"{len(by('resumable'))} · waiting human {len(by('waiting_on_human'))} · suspended {len(by('suspended'))} · "
        f"sealed {len(by('sealed'))}",
    ]
    for st, title in (("resumable", "RESUMABLE"), ("ready", "READY"), ("waiting_on_human", "WAITING ON HUMAN")):
        for d in by(st)[:6]:
            out.append(f"- {title} {d['id']} · {d['resumePriority']} · {d['objective'][:220]}"
                       + (f" · resume: {d['resumeContract'][:200]}" if d.get("resumeContract") else ""))
    out += [
        "",
        "AGENCY · SEMANTIC FREEDOM",
        "TACTICAL FOCUS is advisory, not route authority. Optimize Mission progress rather than cursor progress.",
        "You may DEEPEN, REDIRECT, branch, batch, author/reuse tooling, or change abstraction level when expected "
        "leverage is higher.",
        "Repeated mechanically decidable work should become tooling/batch analysis; preserve unresolved branches and "
        "explicit return contracts.",
        "Choose qualitatively by INFORMATION_GAIN / LEVERAGE / MECHANICALITY / BLOCKING_VALUE / COST.",
        f"STRATEGIC FRONTIER · {f['posture']}",
    ]
    if f["activeTheory"]:
        out.append(f"THEORY: {f['activeTheory']}")
    out.append(f"REMINDER: {f['strategyReminder']}")
    fronts = [x for x in f["fronts"] if x["status"] not in ("CLOSED", "SEALED", "SUPERSEDED")]
    if fronts:
        out.append("FRONTS: " + " · ".join(f"{x['frontId']}={x['title']}" for x in fronts[:5]))
    if at["reviewDue"]:
        out += ["STRATEGY CHECK · repeated low-leverage local pattern detected."] + [f"- {r}" for r in at["reasons"]] + [
            "Before continuing manually, assess whether semantic proof is already closed. If proof is closed but "
            "acceptance/integration housekeeping remains, seal the current Distance and emit nextDistance for that "
            "dependent debt instead of keeping semantic uncertainty open. Otherwise prefer tooling/batch/another "
            "semantic plane. Answer with a strategicFrontier patch (reviewed: true)."]
    out += [
        "",
        "ACTIVE DISTANCE",
        f"id: {a['id']}\nobjective: {a['objective']}\nclosure: {a['closureContract'] or '(not specified)'}" if a else "(none)",
        "",
        "RETURN POINT", json.dumps(s["route"].get("returnPoint"), ensure_ascii=False) if s["route"].get("returnPoint") else "(none)",
        "",
        "RECENT WORKLOG · MIXED-PROVENANCE HISTORY, NOT FACTUAL AUTHORITY",
    ]
    out += [f"- r{w['revision']} · {w['activity']} · effect {w['artifactEffect']} · {w['action']} · {w['summary'][:400]}"
            for w in s["worklog"][-6:]] or ["(empty)"]
    out += ["", "RECENT DECISIONS · ONLY DURABLE CHOICES"]
    out += [f"- {d['text']}" + (f" — {d['reason']}" if d.get("reason") else "") for d in s["decisions"][-6:]] or ["(none)"]
    out += ["", "PROMISE NOTES · ONLY OPEN/USEFUL MEMORY"]
    out += [f"- [{n['kind']}] {n['text']}" for n in s["notes"] if n["status"] != "resolved"][-8:] or ["(none)"]
    hr = [r for r in s["humanRequests"].values() if r.get("status") in ("OPEN", "ANSWERED", "DEFERRED")]
    if hr:
        out += ["", "HUMAN INBOX"]
        for r in hr:
            line = f"- {r['request_id']} · {r['status']} · {r['blocking_class']} · {r.get('question', '')[:300]}"
            if r.get("status") == "ANSWERED":
                line += f"\n  HUMAN_DECISION: {r.get('answer')} — apply it with provenance, then consumeHumanRequests"
            out.append(line)
    if s.get("operatorPrerequisite", {}).get("status") == "waiting":
        out += ["", "OPERATOR PREREQUISITE · WAITING", s["operatorPrerequisite"]["text"]]
    out += [
        "",
        "CONTINUATION NOTE · ADVISORY",
        s["route"].get("tacticalFocus") or "(continue the active Distance according to the actual code)",
        "This is the previous iteration's hint, not a required next step. Continue, deepen, return or redirect by the "
        "actual situation.",
        "",
        "RESULT RETURN",
        f"One JSON move: {{\"baseRevision\": {s['revision']}, \"action\": <{'|'.join(ACTIONS)}>, \"summary\": ..., ...}}",
        "Fields when needed: activity, tacticalFocus, activeDistance, distance (DEEPEN), nextDistance (REDIRECT / "
        "DISTANCE_SEALED), resumeContract, readyDistances, resumeDistances, notes|promises, decisions, "
        "strategicFrontier, humanRequests [{question, blocking_class: BLOCKING|DEFERRED, options, "
        "recommended_option, resume_contract}], consumeHumanRequests, evidence (seals), artifacts (commits/files "
        "this move produced), seal (RETURN), reason.",
        "A refused move changes nothing: read the law error, repair the move in the same causal turn.",
    ]
    return "\n".join(out)


# ---------------------------------------------------------------- CLI

def load(path):
    with open(path, encoding="utf-8") as f:
        s = json.load(f)
    if s.get("schema") != SCHEMA:
        raise SystemExit("Unsupported or missing semantic session")
    return s


def save(path, s):
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(s, f, ensure_ascii=False, indent=1)
        f.write("\n")
    os.replace(tmp, path)


def main(argv):
    if len(argv) < 3:
        print(__doc__)
        return 2
    cmd, path = argv[1], argv[2]
    if cmd == "init":
        args = dict(zip(argv[3::2], argv[4::2]))
        if os.path.exists(path):
            raise SystemExit(f"{path} exists")
        os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
        s = session_new(args.get("--direction", ""), args.get("--reminder", ""),
                        args.get("--objective", ""), args.get("--closure", ""))
        save(path, s)
        print(dispatch(s))
    elif cmd == "dispatch":
        s = load(path)
        reconcile(s)
        print(dispatch(s))
    elif cmd == "apply":
        s = load(path)
        raw = sys.stdin.read() if argv[3] == "-" else open(argv[3], encoding="utf-8").read()
        try:
            s2 = apply(s, json.loads(raw))
        except Law as e:
            print(f"REFUSED · {e}", file=sys.stderr)
            return 1
        save(path, s2)
        a = active(s2)
        print(f"accepted · revision {s2['revision']} · semanticChanged {s2['lastControl']['semanticChanged']} · "
              f"active {a['id'] + ' — ' + a['objective'][:100] if a else 'none'} · status {s2['status']}")
    elif cmd == "answer":
        s = load(path)
        rid, ans = argv[3], " ".join(argv[4:])
        r = s["humanRequests"].get(rid)
        if not r:
            raise SystemExit(f"no request {rid}")
        r.update(status="ANSWERED", answer=ans, answeredAt=now())
        reconcile(s, "human-answered")
        save(path, s)
        print(f"answered {rid}")
    else:
        print(__doc__)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
