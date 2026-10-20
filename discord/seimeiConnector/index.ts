/*
 * seimei connector for Vesktop: answers seimei's requests with Discord's own in-client stores and API.
 * The token stays inside the client: every call goes through RestAPI, which signs it itself.
 * Writes (send, edit, delete, react) only arrive after the user approved a card on the seimei side.
 */

import definePlugin, { PluginNative } from "@utils/types";
import { CloudUpload as TCloudUpload } from "@vencord/discord-types";
import { CloudUploadPlatform } from "@vencord/discord-types/enums";
import { findLazy } from "@webpack";
import {
    ActiveJoinedThreadsStore, ChannelStore, FluxDispatcher, GuildChannelStore, GuildStore, MessageStore,
    ReadStateStore, RestAPI, SnowflakeUtils, UserStore,
} from "@webpack/common";

const Native = VencordNative.pluginHelpers.SeimeiConnector as PluginNative<typeof import("./native")>;
const CloudUpload: typeof TCloudUpload = findLazy(m => m.prototype?.trackUploadFinished);

const MAX_READ = 100;
let running = false;

const userName = (u: any) => u?.globalName || u?.global_name || u?.username || "unknown";
const when = (id: string) => new Date(SnowflakeUtils.extractTimestamp(id)).toISOString();
const id = (v: unknown) => {
    const s = String(v ?? "");
    if (!/^\d{5,25}$/.test(s)) throw new Error(`not a Discord id: ${s}`);
    return s;
};

function channelOf(cid: string) {
    const c = ChannelStore.getChannel(id(cid));
    if (!c) throw new Error("no such channel (or not visible to this account)");
    return c;
}

/** Where a message goes, as the user knows it: "#general in Server", "DM with Ana", "thread x in #general". */
function where(c: any): string {
    if (c.isDM?.() || c.type === 1) return `DM with ${userName(UserStore.getUser(c.recipients?.[0]))}`;
    if (c.isGroupDM?.() || c.type === 3) return `group DM ${c.name || c.recipients?.map((r: string) => userName(UserStore.getUser(r))).join(", ")}`;
    const g = GuildStore.getGuild(c.guild_id);
    if (c.isThread?.() || [10, 11, 12].includes(c.type)) {
        const parent = ChannelStore.getChannel(c.parent_id);
        return `thread "${c.name}" in #${parent?.name ?? "?"} (${g?.name ?? "?"})`;
    }
    return `#${c.name} in ${g?.name ?? "?"}`;
}

function message(m: any) {
    const me = UserStore.getCurrentUser()?.id;
    return {
        id: m.id,
        time: m.timestamp ? new Date(m.timestamp).toISOString() : when(m.id),
        author: userName(m.author),
        own: m.author?.id === me,
        content: m.content ?? "",
        edited: Boolean(m.edited_timestamp ?? m.editedTimestamp),
        attachments: (m.attachments ?? []).map((a: any) => ({ name: a.filename, url: a.url, size: a.size })),
        embeds: (m.embeds ?? []).map((e: any) => e.title || e.url || e.rawTitle).filter(Boolean),
        stickers: (m.sticker_items ?? m.stickerItems ?? []).map((s: any) => s.name),
        reply_to: m.message_reference?.message_id ?? m.messageReference?.message_id ?? null,
        replied: m.referenced_message ? `${userName(m.referenced_message.author)}: ${String(m.referenced_message.content ?? "").slice(0, 120)}` : null,
        thread: m.thread ? { id: m.thread.id, name: m.thread.name } : null,
        reactions: (m.reactions ?? []).map((r: any) => `${r.emoji?.name ?? "?"} ${r.count}`),
    };
}

async function fetchOne(cid: string, mid: string) {
    const cached = MessageStore.getMessage(cid, mid);
    if (cached) return message(cached);
    const r = await RestAPI.get({ url: `/channels/${cid}/messages`, query: { around: mid, limit: 1 } });
    const m = (r.body ?? []).find((x: any) => x.id === mid);
    if (!m) throw new Error("no such message");
    return message(m);
}

function b64ToFile(name: string, data: string, type: string) {
    const bin = atob(data);
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    return new File([bytes], name, { type: type || "application/octet-stream" });
}

function upload(cid: string, file: File): Promise<{ filename: string; uploadedFilename: string; }> {
    return new Promise((resolve, reject) => {
        const up = new CloudUpload({ file, isThumbnail: false, platform: CloudUploadPlatform.WEB }, cid);
        up.on("complete", () => resolve({ filename: up.filename, uploadedFilename: up.uploadedFilename }));
        up.on("error", () => reject(new Error(`upload of ${file.name} failed`)));
        up.upload();
    });
}

const OPS: Record<string, (a: any) => Promise<any> | any> = {
    guilds({ guild }) {
        if (!guild) {
            return Object.values(GuildStore.getGuilds()).map((g: any) => ({
                id: g.id, name: g.name,
                unread: GuildChannelStore.getChannels(g.id).SELECTABLE.filter((c: any) => ReadStateStore.hasUnread(c.channel.id)).length,
            }));
        }
        const g = GuildStore.getGuild(id(guild));
        if (!g) throw new Error("no such server");
        const chans = GuildChannelStore.getChannels(g.id).SELECTABLE.map((c: any) => c.channel);
        const threads = Object.values(ActiveJoinedThreadsStore.getActiveJoinedThreadsForGuild(g.id) ?? {})
            .flatMap((byParent: any) => Object.values(byParent)).map((t: any) => t.channel ?? t);
        return {
            id: g.id, name: g.name,
            channels: chans.map((c: any) => ({
                id: c.id, name: c.name, category: ChannelStore.getChannel(c.parent_id)?.name ?? "",
                unread: ReadStateStore.hasUnread(c.id),
            })),
            threads: threads.map((t: any) => ({
                id: t.id, name: t.name, parent: ChannelStore.getChannel(t.parent_id)?.name ?? "", unread: ReadStateStore.hasUnread(t.id),
            })),
        };
    },
    dms({ limit }) {
        return ChannelStore.getSortedPrivateChannels().slice(0, Math.min(Number(limit) || 30, 100)).map((c: any) => ({
            id: c.id, name: where(c), unread: ReadStateStore.hasUnread(c.id),
            last: c.lastMessageId ? when(c.lastMessageId) : null,
        }));
    },
    where({ channel }) {
        return where(channelOf(channel));
    },
    async read({ channel, limit, before }) {
        const c = channelOf(channel);
        const query: any = { limit: Math.max(1, Math.min(Number(limit) || 30, MAX_READ)) };
        if (before) query.before = id(before);
        const r = await RestAPI.get({ url: `/channels/${c.id}/messages`, query });
        return { where: where(c), messages: (r.body ?? []).map(message) };
    },
    message({ channel, message: mid }) {
        const c = channelOf(channel);
        return fetchOne(c.id, id(mid)).then(m => ({ ...m, where: where(c) }));
    },
    async search({ query, guild, channel }) {
        const q: any = { content: String(query ?? "").slice(0, 200) };
        let url: string;
        if (guild) {
            url = `/guilds/${id(guild)}/messages/search`;
            if (channel) q.channel_id = id(channel);
        } else if (channel) url = `/channels/${channelOf(channel).id}/messages/search`;
        else throw new Error("give a server or a channel to search in");
        for (let tries = 0; tries < 3; tries++) {
            const r = await RestAPI.get({ url, query: q });
            // 202: Discord is still indexing; it says how long to wait.
            if (r.status === 202) {
                await new Promise(res => setTimeout(res, Math.min(5000, (r.body?.retry_after ?? 2) * 1000)));
                continue;
            }
            return {
                total: r.body?.total_results ?? 0,
                messages: (r.body?.messages ?? []).map((group: any[]) => {
                    const m = group.find((x: any) => x.hit) ?? group[0];
                    return { ...message(m), where: where(ChannelStore.getChannel(m.channel_id) ?? { name: "?", guild_id: guild }), channel: m.channel_id };
                }),
            };
        }
        throw new Error("Discord is still indexing that search; try again in a bit");
    },
    async send({ channel, text, reply_to, files }) {
        const c = channelOf(channel);
        const attachments: any[] = [];
        for (const [i, f] of (files ?? []).entries()) {
            const up = await upload(c.id, b64ToFile(f.name, f.data, f.type));
            attachments.push({ id: String(i), filename: up.filename, uploaded_filename: up.uploadedFilename });
        }
        const body: any = { content: String(text ?? ""), nonce: SnowflakeUtils.fromTimestamp(Date.now()), tts: false, flags: 0 };
        if (attachments.length) body.attachments = attachments;
        if (reply_to) body.message_reference = { message_id: id(reply_to), channel_id: c.id, guild_id: c.guild_id ?? undefined, fail_if_not_exists: false };
        const r = await RestAPI.post({ url: `/channels/${c.id}/messages`, body });
        return { id: r.body?.id, where: where(c) };
    },
    async edit({ channel, message: mid, text }) {
        const c = channelOf(channel);
        const m = await fetchOne(c.id, id(mid));
        if (!m.own) throw new Error("only your own messages can be edited");
        await RestAPI.patch({ url: `/channels/${c.id}/messages/${m.id}`, body: { content: String(text ?? "") } });
        return { id: m.id };
    },
    async delete({ channel, message: mid }) {
        const c = channelOf(channel);
        const m = await fetchOne(c.id, id(mid));
        if (!m.own) throw new Error("only your own messages can be deleted");
        await RestAPI.del({ url: `/channels/${c.id}/messages/${m.id}` });
        return { id: m.id };
    },
    async react({ channel, message: mid, emoji }) {
        const c = channelOf(channel);
        const e = String(emoji ?? "").trim();
        if (!e || e.length > 100) throw new Error("give one emoji (unicode, or name:id for a custom one)");
        await RestAPI.put({ url: `/channels/${c.id}/messages/${id(mid)}/reactions/${encodeURIComponent(e)}/@me` });
        return { id: mid };
    },
    ack({ channel }) {
        const c = channelOf(channel);
        FluxDispatcher.dispatch({
            type: "BULK_ACK", context: "APP",
            channels: [{ channelId: c.id, messageId: ReadStateStore.lastMessageId(c.id), readStateType: 0 }],
        });
        return where(c);
    },
};

async function handle(m: any) {
    let out: any;
    try {
        const op = OPS[m.op];
        if (!op) throw new Error(`unknown op ${m.op}`);
        out = { id: m.id, ok: true, result: await op(m.args ?? {}) };
    } catch (e: any) {
        // RestAPI rejects with the response; keep only Discord's message, never headers.
        const msg = e?.body?.message ?? e?.message ?? String(e);
        out = { id: m.id, ok: false, error: String(msg).slice(0, 500) };
    }
    await Native.reply(out);
}

async function loop() {
    await Native.start();
    while (running) {
        const m = await Native.next().catch(() => null);
        if (m && running) handle(m);
    }
}

export default definePlugin({
    name: "SeimeiConnector",
    description: "Lets seimei on this laptop read your servers and DMs and, after you approve each one, send, edit, delete or react.",
    authors: [{ name: "hisamiii", id: 0n }],
    start() {
        running = true;
        loop();
    },
    stop() {
        running = false;
        Native.stop();
    },
});
