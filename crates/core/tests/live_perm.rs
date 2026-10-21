//! The live permtest on the Rust pool (tests/live/permtest.py is the Python one; its offline half is
//! tests/approvals.rs): permission modes and cards with haiku shikigami in a scratch folder, a scripted "user"
//! answering the cards, nothing reaching a phone. Run by tests/live/permtest_rs.py.

mod live;

use std::sync::{Arc, Mutex};

use live::{Checks, pool, transcript, until, work};
use seimei_core::approvals::Approvals;
use seimei_core::events::EventLog;
use seimei_core::workers::{Summon, Worker};
use serde_json::Value;

fn summon(name: &str, task: &str, cwd: &str, mode: Option<&str>) -> Summon {
    Summon {
        name: name.into(),
        task: task.into(),
        cwd: cwd.into(),
        model: "haiku".into(),
        effort: None,
        context: "400k".into(),
        subagents: "haiku".into(),
        project: None,
        link: None,
        tools: vec![],
        mode: mode.map(String::from),
    }
}

fn ended(w: &Worker) -> bool {
    ["done", "failed", "stopped"].contains(&w.state().as_str())
}

/// The scripted user: Always allow for touch, a deny with a note for rm a.txt, once for the rest; no options, deny.
fn answer(a: &Approvals, e: &Value) {
    let aid = e["aid"].as_str().unwrap_or("");
    let d = e["detail"].as_str().unwrap_or("");
    let options: Vec<&Value> = e["options"].as_array().into_iter().flatten().collect();
    if options.is_empty() {
        a.decide(aid, false, None, None);
    } else if d.contains("touch a.txt") {
        let always = options.iter().find(|o| o["label"].as_str().unwrap_or("").starts_with("Always allow"));
        a.decide(aid, true, always.and_then(|o| o["id"].as_str()).map(String::from), None);
    } else if d.starts_with("rm ") && d.contains("a.txt") {
        a.decide(aid, false, Some("deny".into()), Some("keep a.txt, it is needed".into()));
    } else {
        a.decide(aid, true, Some("once".into()), None);
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "starts headless claude sessions; run by tests/live/permtest_rs.py"]
async fn permissions_live() {
    let c = Checks::default();
    let work = work("t64rs");
    let cwd = work.display().to_string();
    let state = live::state("permtest");
    let events = Arc::new(EventLog::open(&state.join("events.jsonl")).unwrap());
    let approvals = Arc::new(Approvals::new(events.clone()));
    let (pool, heard) = pool(&state.join("a"), "default", &events, &approvals);

    let cards = Arc::new(Mutex::new(Vec::<Value>::new()));
    let (ev, a, seen) = (events.clone(), approvals.clone(), cards.clone());
    let user = tokio::spawn(async move {
        loop {
            let mut q = ev.subscribe();
            while let Some(e) = q.recv().await {
                if e["type"] == "approval" {
                    seen.lock().unwrap().push((*e).clone());
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    answer(&a, &e);
                }
            }
        }
    });
    let n_cards = || cards.lock().unwrap().len();
    let since = |n: usize| cards.lock().unwrap()[n..].to_vec();

    let e = pool.summon(summon("pb", "x", &cwd, Some("bypassPermissions"))).await.err().unwrap_or_default();
    c.check("summon can't go above the ceiling", e.contains("only the user"), &e);

    // 1. default mode: cards with options; Always allow writes the rule; a deny note reaches the model.
    let task = "Use your tools, one call at a time: 1) Bash `touch a.txt`; 2) Write b.txt containing hi; 3) Bash \
                `rm a.txt`; 4) Bash `cat ~/.ssh/id_ed25519`. Then reply with one line per step saying what happened, \
                quoting any message you got.";
    let w = pool.summon(summon("perm", task, &cwd, None)).await.unwrap();
    c.check("info shows the mode", pool.info(&w.rec())["mode"] == "default", pool.info(&w.rec()));
    until(240, || ended(&w)).await;
    println!("report: {}", w.get("summary").as_str().unwrap_or("").chars().take(800).collect::<String>());
    let all = since(0);
    let first = all.iter().find(|c| c["detail"].as_str().unwrap_or("").contains("touch a.txt"));
    let offers_always = first.is_some_and(|f| {
        f["options"].as_array().into_iter().flatten().any(|o| {
            o["label"].as_str().unwrap_or("").starts_with("Always allow touch a.txt")
                && o["rule"].as_str().unwrap_or("").starts_with("touch a.txt")
        })
    });
    c.check("touch asked with Claude Code's choices", offers_always, first);
    c.check("the card names the worker", first.is_some_and(|f| f["worker"] == w.id && f["who"] == "perm"), first);
    let local = work.join(".claude/settings.local.json");
    let rules = || -> Vec<String> {
        let v: Value =
            std::fs::read_to_string(&local).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        v["permissions"]["allow"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| r.as_str().map(String::from))
            .collect()
    };
    let touch_rule = || rules().iter().any(|r| r.starts_with("Bash(touch a.txt"));
    c.check("Always allow wrote the rule like Claude Code does", touch_rule(), rules());
    let targets: Vec<&Value> = all.iter().map(|c| &c["target"]).collect();
    c.check(
        "Write asked too and was allowed",
        work.join("b.txt").exists() && all.iter().any(|c| c["action"] == "edit"),
        targets,
    );
    c.check("the deny kept a.txt", work.join("a.txt").exists(), "");
    let t = transcript(&work, w.get("session_id").as_str().unwrap_or(""));
    let said = std::fs::read_to_string(&t).unwrap_or_default();
    c.check("the deny note reached it", said.contains("They said: keep a.txt, it is needed"), &t);
    let details: Vec<&Value> = all.iter().map(|c| &c["detail"]).collect();
    let asked_key = all.iter().any(|c| c["detail"].as_str().unwrap_or("").contains("ssh"));
    c.check("the guard blocked the key read before any card", !asked_key, details);
    let log = events.before(None, 5000);
    let chose_touch = log
        .iter()
        .any(|e| e["type"] == "approval_done" && e["chosen"].as_str().is_some_and(|s| s.contains("touch a.txt")));
    c.check("the done event names the chosen rule", chose_touch, "");
    let open: Vec<&str> = (log.iter())
        .filter(|e| e["type"] == "approval")
        .filter_map(|e| e["aid"].as_str())
        .filter(|aid| !log.iter().any(|d| d["type"] == "approval_done" && d["aid"] == *aid))
        .collect();
    c.check(
        "no card left open, card cleared on the worker",
        open.is_empty() && !seimei_core::util::truthy(&w.get("card")),
        (&open, w.get("card")),
    );
    let lines = heard.0.lock().unwrap().clone();
    let tail = lines[lines.len().saturating_sub(5)..].to_vec();
    c.check("seimei heard it waited on a card", lines.iter().any(|l| l.contains("waits on a permission card")), tail);

    // 2. The user raises its mode on the running session: an edit no longer asks.
    let n = n_cards();
    w.set_mode("acceptEdits").await.unwrap();
    w.send("Write e.txt containing hi, then reply done.", None, vec![]).await.unwrap();
    until(180, || ended(&w) && work.join("e.txt").exists()).await;
    c.check("after Allow edits, Write asks nothing", work.join("e.txt").exists() && n_cards() == n, since(n));
    c.check("its mode shows the change", pool.info(&w.rec())["mode"] == "acceptEdits", pool.info(&w.rec()));
    // Claude Code's settings take a guard card even with edits allowed (auth audit P3); the scripted user denies it.
    let n = n_cards();
    let ask = "Use the Write tool to create .claude/settings.local.json with the content {}, then reply done.";
    w.send(ask, None, vec![]).await.unwrap();
    until(180, || ended(&w) && n_cards() > n).await;
    let new = since(n);
    let guard_card = new.iter().any(|c| {
        c["target"] == "change Claude Code's permission settings"
            && c["options"].as_array().is_none_or(|o| o.is_empty())
    });
    c.check("writing its own settings takes a guard card", guard_card, &new);
    c.check("the denied write left the settings alone", touch_rule(), rules());

    // 3. plan mode: the plan card, Go ahead switches to the ceiling, the edit then asks as usual.
    let n = n_cards();
    let task = "Make a file c.txt containing hi. Plan it first and exit plan mode with the plan, then do it.";
    let p = pool.summon(summon("plan", task, &cwd, Some("plan"))).await.unwrap();
    until(240, || ended(&p) && p.mode() != "plan").await;
    until(60, || work.join("c.txt").exists()).await;
    let new = since(n);
    let go_ahead = new.iter().any(|c| {
        c["action"] == "plan" && c["options"].as_array().into_iter().flatten().any(|o| o["label"] == "Go ahead")
    });
    let shown: Vec<(Value, Value)> = new.iter().map(|c| (c["action"].clone(), c["options"].clone())).collect();
    c.check("plan card with Go ahead", go_ahead, shown);
    c.check("Go ahead set the ceiling mode", p.get("mode") == "default", p.get("mode"));
    c.check(
        "then the write asked and happened",
        work.join("c.txt").exists() && new.iter().any(|c| c["action"] == "edit"),
        "",
    );

    // 4. bypass: no card at all (a pool whose ceiling is bypass, as config sets it).
    let n = n_cards();
    let (bypass, _) = live::pool(&state.join("b"), "bypassPermissions", &events, &approvals);
    let b =
        bypass.summon(summon("byp", "Run the bash command `touch d.txt`, then reply done.", &cwd, None)).await.unwrap();
    until(180, || ended(&b)).await;
    c.check("bypass: no card, the command ran", work.join("d.txt").exists() && n_cards() == n, since(n));

    user.abort();
    c.done();
}
