// Satur8 for GNOME Shell (B4).
//
// Adds a saturation (digital vibrance) shader at the shell level and exposes a
// small D-Bus interface the `satur8` tool drives. Because it is a compositor
// shader applied to already-rendered pixels, it never touches the game process
// (VAC-safe, same category as the KWin effect) and is GPU-agnostic - it works on
// NVIDIA Wayland too, where gamescope would otherwise be the only option.
//
// GNOME 45+ ESM extension, built on Clutter.ShaderEffect. Verified on real
// hardware (GNOME Shell 50.2, NVIDIA, Wayland): the shader desaturates and
// boosts the whole shell, confirmed by screenshot across the saturation range.

import GObject from 'gi://GObject';
import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const DBUS_IFACE = `
<node>
  <interface name="org.satur8.GnomeShell">
    <method name="SetSaturation">
      <arg type="d" name="saturation" direction="in"/>
    </method>
    <method name="Reset"/>
    <property name="Saturation" type="d" access="read"/>
  </interface>
</node>`;

// Same math as every other backend: out = mix(luma, color, s). cogl_color_out /
// cogl_tex_coord_in / cogl_sampler are the Cogl globals a ShaderEffect binds.
const SHADER = `
uniform sampler2D tex;
uniform float satur8;
void main() {
    vec4 c = texture2D(tex, cogl_tex_coord_in[0].st);
    float luma = dot(c.rgb, vec3(0.2126, 0.7152, 0.0722));
    cogl_color_out = vec4(clamp(mix(vec3(luma), c.rgb, satur8), 0.0, 1.0), c.a);
}`;

// Mutter 18 (GNOME 50) dropped the Clutter.ShaderType enum from its GI binding,
// but ShaderEffect's `shader-type` construct property still takes the raw enum
// value. 1 = CLUTTER_FRAGMENT_SHADER (0 = vertex); the literal is stable across
// every Clutter version, so this works on GNOME 45 through 50.
const CLUTTER_FRAGMENT_SHADER = 1;

const Satur8Effect = GObject.registerClass(
class Satur8Effect extends Clutter.ShaderEffect {
    _init(saturation) {
        super._init({shader_type: CLUTTER_FRAGMENT_SHADER});
        this.set_shader_source(SHADER);
        this.setSaturation(saturation);
    }

    setSaturation(saturation) {
        this.set_uniform_value('tex', 0);
        this.set_uniform_value('satur8', parseFloat(saturation));
    }

    vfunc_get_static_shader_source() {
        return SHADER;
    }
});

export default class Satur8Extension extends Extension {
    enable() {
        this._saturation = 1.0;
        this._effect = null;
        this._dbus = Gio.DBusExportedObject.wrapJSObject(DBUS_IFACE, this);
        this._dbus.export(Gio.DBus.session, '/org/satur8/GnomeShell');
        this._owner = Gio.bus_own_name(
            Gio.BusType.SESSION, 'org.satur8.GnomeShell',
            Gio.BusNameOwnerFlags.NONE, null, null, null);
    }

    disable() {
        this._removeEffect();
        if (this._dbus) {
            this._dbus.unexport();
            this._dbus = null;
        }
        if (this._owner) {
            Gio.bus_unown_name(this._owner);
            this._owner = 0;
        }
    }

    // The actor that carries everything on screen, including game windows.
    get _target() {
        return Main.uiGroup;
    }

    _ensureEffect() {
        if (!this._effect) {
            this._effect = new Satur8Effect(this._saturation);
            this._target.add_effect_with_name('satur8', this._effect);
        }
    }

    _removeEffect() {
        if (this._effect) {
            this._target.remove_effect(this._effect);
            this._effect = null;
        }
    }

    // ---- D-Bus surface ----
    SetSaturation(saturation) {
        this._saturation = saturation;
        if (Math.abs(saturation - 1.0) < 1e-4) {
            this._removeEffect();
            return;
        }
        this._ensureEffect();
        this._effect.setSaturation(saturation);
        // A uniform-only change can reuse the effect's cached offscreen buffer,
        // so on an idle desktop the new saturation would not show until some
        // other repaint happened. queue_repaint() forces the effect to re-run.
        this._effect.queue_repaint();
    }

    Reset() {
        this._saturation = 1.0;
        this._removeEffect();
    }

    get Saturation() {
        return this._saturation;
    }
}
