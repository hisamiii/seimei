// seimei's GNOME Shell extension: what the seimei desktop plugin cannot get from outside the shell on Wayland (the
// window list, focus, workspaces, screenshots, input), as one D-Bus method on the session bus. Call(s) -> s takes and
// returns JSON: {"op": ...} in, the answer or {"error": ...} out. Nothing runs until a call comes.

import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
import Shell from 'gi://Shell';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

const IFACE = `<node><interface name="org.seimei.Desktop">
  <method name="Call"><arg type="s" direction="in" name="request"/><arg type="s" direction="out" name="reply"/></method>
</interface></node>`;

const KINDS = [Meta.WindowType.NORMAL, Meta.WindowType.DIALOG, Meta.WindowType.MODAL_DIALOG, Meta.WindowType.UTILITY];
const MODS = {ctrl: 0xffe3, control: 0xffe3, shift: 0xffe1, alt: 0xffe9, super: 0xffeb, meta: 0xffeb, win: 0xffeb};

/** Meta's window ids are long; seimei's are small numbers given in the order windows are first seen. */
const ids = new Map();
let nextId = 1;

function idOf(w) {
    const key = w.get_id();
    if (!ids.has(key))
        ids.set(key, nextId++);
    return ids.get(key);
}

function now() {
    return global.get_current_time();
}

function listed() {
    return global.get_window_actors().map(a => a.meta_window)
        .filter(w => KINDS.includes(w.get_window_type()) && !w.is_skip_taskbar());
}

function find(id) {
    const w = listed().find(x => idOf(x) === id);
    if (!w)
        throw new Error(`no window ${id}`);
    return w;
}

function workspaceOf(w) {
    const wm = global.workspace_manager;
    const ws = w.is_on_all_workspaces() ? wm.get_active_workspace() : w.get_workspace();
    return ws ? ws.index() + 1 : null;
}

function windows() {
    const focus = global.display.get_focus_window();
    // Bottom to top, as the shell stacks them.
    const order = global.display.sort_windows_by_stacking(listed());
    return order.map((w, i) => {
        const r = w.get_frame_rect();
        return {
            id: idOf(w), app_id: w.get_wm_class() || w.get_gtk_application_id() || '', title: w.get_title() || '',
            pid: w.get_pid(), workspace_id: workspaceOf(w), is_focused: w === focus, is_urgent: w.urgent || false,
            is_floating: true, is_minimized: w.minimized, pos: [i],
            rect: {x: r.x, y: r.y, width: r.width, height: r.height},
        };
    });
}

function names() {
    return new Gio.Settings({schema_id: 'org.gnome.desktop.wm.preferences'}).get_strv('workspace-names');
}

/** Names seimei gave, on the workspace objects: GNOME drops empty workspaces, which shifts the settings' names. */
const named = new Map();

function nameOf(ws, i, n) {
    for (const [name, w] of named) {
        if (w === ws)
            return name;
    }
    return n[i] || null;
}

function byName(name) {
    const wm = global.workspace_manager;
    const ws = named.get(name);
    for (let i = 0; ws && i < wm.get_n_workspaces(); i++) {
        if (wm.get_workspace_by_index(i) === ws)
            return ws;
    }
    named.delete(name);
    const i = names().indexOf(name);
    return i >= 0 && i < wm.get_n_workspaces() ? wm.get_workspace_by_index(i) : null;
}

function workspaces() {
    const wm = global.workspace_manager;
    const focus = global.display.get_focus_window();
    const n = names();
    const out = [];
    for (let i = 0; i < wm.get_n_workspaces(); i++) {
        const active = i === wm.get_active_workspace_index();
        out.push({
            id: i + 1, idx: i + 1, name: nameOf(wm.get_workspace_by_index(i), i, n), output: null, is_active: active,
            active_window_id: active && focus ? idOf(focus) : null,
        });
    }
    return out;
}

function outputs() {
    const d = global.display;
    const out = {};
    for (let i = 0; i < d.get_n_monitors(); i++) {
        const g = d.get_monitor_geometry(i);
        out[`monitor-${i}`] = {x: g.x, y: g.y, width: g.width, height: g.height, scale: d.get_monitor_scale(i)};
    }
    return out;
}

/** Move a window to the workspace with this number, or with this name: the empty last one (GNOME keeps one) or a
 * new one gets the name when none has it. */
function toWorkspace(w, which) {
    const wm = global.workspace_manager;
    const idx = Number(which) - 1;
    if (Number.isInteger(idx) && idx >= 0) {
        if (idx >= wm.get_n_workspaces())
            throw new Error(`no workspace ${which}`);
        w.change_workspace_by_index(idx, false);
        return;
    }
    const name = String(which);
    let ws = byName(name);
    if (!ws) {
        const last = wm.get_workspace_by_index(wm.get_n_workspaces() - 1);
        const empty = last.list_windows().every(x => x === w || !KINDS.includes(x.get_window_type()));
        ws = empty && ![...named.values()].includes(last) ? last : wm.append_new_workspace(false, now());
        named.set(name, ws);
    }
    w.change_workspace(ws);
}

function toggle(on, get, set, unset) {
    const want = on === undefined ? !get() : on;
    if (want)
        set();
    else
        unset();
}

function act(args) {
    const [name, a1, a2] = args;
    const id = Number(a1);
    const onOff = a2 === 'on' ? true : a2 === 'off' ? false : undefined;
    switch (name) {
    case 'activate':
        Main.activateWindow(find(id));
        return;
    case 'close':
        find(id).delete(now());
        return;
    case 'minimize': {
        const w = find(id);
        toggle(onOff, () => w.minimized, () => w.minimize(), () => w.unminimize());
        return;
    }
    case 'maximize': {
        const w = find(id);
        toggle(onOff, () => w.get_maximized() === Meta.MaximizeFlags.BOTH,
            () => w.maximize(Meta.MaximizeFlags.BOTH), () => w.unmaximize(Meta.MaximizeFlags.BOTH));
        return;
    }
    case 'fullscreen': {
        const w = find(id);
        toggle(onOff, () => w.is_fullscreen(), () => w.make_fullscreen(), () => w.unmake_fullscreen());
        return;
    }
    case 'above': {
        const w = find(id);
        toggle(onOff, () => w.is_above(), () => w.make_above(), () => w.unmake_above());
        return;
    }
    case 'to-workspace':
        toWorkspace(find(id), a2);
        return;
    case 'workspace': {
        const wm = global.workspace_manager;
        const i = Number(a1) - 1;
        const ws = Number.isInteger(i) ? wm.get_workspace_by_index(i) : null;
        if (!ws)
            throw new Error(`no workspace ${a1}`);
        ws.activate(now());
        return;
    }
    default:
        throw new Error(`unknown action ${name}; one of activate, close, minimize, maximize, fullscreen, above ` +
            '(window [on|off]), to-workspace (window, number or name), workspace (number)');
    }
}

/** A screenshot written as PNG to path: the screen, an area {x, y, width, height}, or one window by id. */
function shoot(req) {
    const file = Gio.File.new_for_path(req.path);
    const stream = file.replace(null, false, Gio.FileCreateFlags.PRIVATE, null);
    return new Promise((resolve, reject) => {
        const done = (source, res, name) => {
            try {
                source[`${name}_finish`](res);
                stream.close(null);
                resolve({path: req.path});
            } catch (e) {
                reject(e);
            }
        };
        const shooter = new Shell.Screenshot();
        if (req.window !== undefined) {
            // The window's own pixels, also when it is on another workspace or covered.
            const actor = find(req.window).get_compositor_private();
            const texture = actor.paint_to_content(null).get_texture();
            Shell.Screenshot.composite_to_stream(texture, 0, 0, -1, -1, 1, null, 0, 0, 1, stream,
                (s, res) => done(Shell.Screenshot, res, 'composite_to_stream'));
        } else if (req.area) {
            const {x, y, width, height} = req.area;
            shooter.screenshot_area(x, y, width, height, stream, (s, res) => done(s, res, 'screenshot_area'));
        } else {
            shooter.screenshot(false, stream, (s, res) => done(s, res, 'screenshot'));
        }
    });
}

function sleep(ms) {
    return new Promise(resolve => GLib.timeout_add(GLib.PRIORITY_DEFAULT, ms, () => {
        resolve();
        return GLib.SOURCE_REMOVE;
    }));
}

class Input {
    constructor() {
        // GNOME 46 and later reach the backend through the stage; older ones through Clutter.
        const backend = global.stage.context?.get_backend?.() ?? Clutter.get_default_backend();
        const seat = backend.get_default_seat();
        this.kb = seat.create_virtual_device(Clutter.InputDeviceType.KEYBOARD_DEVICE);
        this.ptr = seat.create_virtual_device(Clutter.InputDeviceType.POINTER_DEVICE);
        // A seat without devices (a headless shell) gains its keyboard and pointer now; clients pick them up a moment
        // later, and the focused window gets its keyboard only when focus comes again.
        this.ready = sleep(300).then(() => {
            const w = global.display.get_focus_window();
            if (w) {
                global.display.unset_input_focus(now());
                w.activate(now());
            }
            return sleep(50);
        });
    }

    key(sym, down) {
        this.kb.notify_keyval(GLib.get_monotonic_time(), sym,
            down ? Clutter.KeyState.PRESSED : Clutter.KeyState.RELEASED);
    }

    /** Put the pointer at x, y. A new device's first motion can land with y at 0 (headless GNOME 51), so look and
     * move again when it missed. */
    async move(x, y) {
        for (let i = 0; i < 3; i++) {
            this.ptr.notify_absolute_motion(GLib.get_monotonic_time(), x, y);
            await sleep(20);
            const [px, py] = global.get_pointer();
            if (Math.abs(px - x) < 1 && Math.abs(py - y) < 1)
                return;
        }
    }

    /** Events in order: ["key", keysym, [modifier names]], ["text", s], ["click", x, y, button, count], ["scroll",
     * x, y, amount]. Text goes through the input method when a text field has it, as keysyms otherwise. */
    async run(events) {
        await this.ready;
        const t = () => GLib.get_monotonic_time();
        for (const e of events) {
            if (e[0] === 'key') {
                const mods = (e[2] || []).map(m => MODS[m]).filter(Boolean);
                mods.forEach(m => this.key(m, true));
                this.key(e[1], true);
                this.key(e[1], false);
                mods.reverse().forEach(m => this.key(m, false));
            } else if (e[0] === 'text') {
                const im = Main.inputMethod;
                if (im && im._currentFocus) {
                    im.commit(e[1]);
                } else {
                    for (const c of e[1]) {
                        const sym = Clutter.unicode_to_keysym(c.codePointAt(0));
                        this.key(sym, true);
                        this.key(sym, false);
                        await sleep(2);
                    }
                }
            } else if (e[0] === 'click') {
                await this.move(e[1], e[2]);
                for (let i = 0; i < e[4]; i++) {
                    this.ptr.notify_button(t(), e[3], Clutter.ButtonState.PRESSED);
                    await sleep(10);
                    this.ptr.notify_button(t(), e[3], Clutter.ButtonState.RELEASED);
                    await sleep(10);
                }
            } else if (e[0] === 'scroll') {
                await this.move(e[1], e[2]);
                const dir = e[3] > 0 ? Clutter.ScrollDirection.DOWN : Clutter.ScrollDirection.UP;
                for (let i = 0; i < Math.abs(e[3]); i++)
                    this.ptr.notify_discrete_scroll(t(), dir, Clutter.ScrollSource.WHEEL);
            }
            await sleep(5);
        }
    }
}

export default class SeimeiExtension extends Extension {
    enable() {
        this._input = null;
        this._dbus = Gio.DBusExportedObject.wrapJSObject(IFACE, this);
        this._dbus.export(Gio.DBus.session, '/org/seimei/Desktop');
        this._name = Gio.bus_own_name_on_connection(Gio.DBus.session, 'org.seimei.Desktop',
            Gio.BusNameOwnerFlags.NONE, null, null);
    }

    disable() {
        Gio.bus_unown_name(this._name);
        this._dbus.unexport();
        this._dbus = null;
        this._input = null;
    }

    async CallAsync(params, invocation) {
        let out;
        try {
            out = await this._call(JSON.parse(params[0]));
        } catch (e) {
            out = {error: String(e.message || e)};
        }
        invocation.return_value(new GLib.Variant('(s)', [JSON.stringify(out ?? null)]));
    }

    async _call(req) {
        switch (req.op) {
        case 'windows':
            return windows();
        case 'workspaces':
            return workspaces();
        case 'outputs':
            return outputs();
        case 'locked':
            return Boolean(Main.screenShield && Main.screenShield.locked);
        case 'act':
            act(req.args);
            return null;
        case 'shot':
            return await shoot(req);
        case 'input':
            this._input ??= new Input();
            await this._input.run(req.events);
            return null;
        default:
            throw new Error(`unknown op ${req.op}`);
        }
    }
}
