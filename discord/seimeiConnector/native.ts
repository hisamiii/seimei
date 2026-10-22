/*
 * SPDX-License-Identifier: GPL-3.0-or-later
 * seimei connector for Vesktop: the main-process side.
 * It keeps one connection to the seimei server's unix socket (mode 600 in the runtime dir) and hands its requests to
 * the client side by long polling. It never sees or sends the Discord token.
 */

import { createConnection, Socket } from "net";
import { join } from "path";

const SOCK = process.env.SEIMEI_DISCORD_SOCK
    || join(process.env.XDG_RUNTIME_DIR || `/run/user/${process.getuid?.()}`, "seimei", "discord.sock");

let sock: Socket | null = null;
let buf = "";
let started = false;
let retry: ReturnType<typeof setTimeout> | null = null;
const queue: any[] = [];
let waiter: ((m: any) => void) | null = null;

function deliver(m: any) {
    if (waiter) {
        const w = waiter;
        waiter = null;
        w(m);
    } else queue.push(m);
}

function connect() {
    const s = createConnection(SOCK);
    s.setEncoding("utf8");
    s.on("connect", () => { sock = s; });
    s.on("data", (d: string) => {
        buf += d;
        let i: number;
        while ((i = buf.indexOf("\n")) >= 0) {
            const line = buf.slice(0, i);
            buf = buf.slice(i + 1);
            if (!line.trim()) continue;
            try {
                deliver(JSON.parse(line));
            } catch { }
        }
    });
    s.on("error", () => { });
    s.on("close", () => {
        if (sock === s) sock = null;
        buf = "";
        queue.length = 0;
        if (started) retry = setTimeout(connect, 3000);
    });
}

export function start(_: unknown) {
    if (started) return;
    started = true;
    connect();
}

/** Plugin turned off: drop the link, so seimei sees Discord as gone instead of waiting on unanswered calls. */
export function stop(_: unknown) {
    started = false;
    if (retry) clearTimeout(retry);
    retry = null;
    sock?.destroy();
    sock = null;
    if (waiter) {
        const w = waiter;
        waiter = null;
        w(null);
    }
}

/** The next request from seimei, or null after a while so the client side can loop (and a reload loses nothing). */
export function next(_: unknown): Promise<any> {
    if (queue.length) return Promise.resolve(queue.shift());
    if (waiter) waiter(null);
    return new Promise(resolve => {
        waiter = resolve;
        setTimeout(() => {
            if (waiter === resolve) {
                waiter = null;
                resolve(null);
            }
        }, 20000);
    });
}

export function reply(_: unknown, msg: any) {
    sock?.write(JSON.stringify(msg) + "\n");
}
