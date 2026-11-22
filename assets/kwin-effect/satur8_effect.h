/*
    Satur8 - a single-purpose KWin saturation effect.

    This is the compositor half of the KWin (B1) backend. It redirects windows
    into an offscreen texture and runs one tiny GLSL pass that boosts saturation
    (digital vibrance). It never touches the game process - it only re-colours
    pixels the game has already rendered and handed to the compositor, so it is
    outside the scope of anti-cheat (the same category as a monitor's saturation
    OSD).

    SPDX-License-Identifier: GPL-3.0-or-later
*/

#pragma once

#include <effect/offscreeneffect.h>

#include <QHash>
#include <QString>
#include <QVariantMap>
#include <QVector3D>
#include <memory>

namespace KWin
{
class GLShader;
class EffectWindow;
}

// KWin 6.8 changed Effect::drawWindow from void to bool.
#if SATUR8_KWIN_MAJOR > 6 || (SATUR8_KWIN_MAJOR == 6 && SATUR8_KWIN_MINOR >= 8)
using Satur8DrawResult = bool;
#else
using Satur8DrawResult = void;
#endif

class Satur8Effect : public KWin::OffscreenEffect
{
    Q_OBJECT
    Q_CLASSINFO("D-Bus Interface", "org.kde.kwin.Effect.Satur8")

public:
    Satur8Effect();
    ~Satur8Effect() override;

    static bool supported();

    void reconfigure(ReconfigureFlags flags) override;
    int requestedEffectChainPosition() const override
    {
        return 99; // late: act on the final, composited look
    }
    bool isActive() const override;

protected:
    Satur8DrawResult drawWindow(const KWin::RenderTarget &renderTarget,
                    const KWin::RenderViewport &viewport,
                    KWin::EffectWindow *w,
                    int mask,
                    const KWin::Region &deviceRegion,
                    KWin::WindowPaintData &data) override;

public Q_SLOTS:
    /// Live saturation control over D-Bus. 1.0 = unchanged, >1 = more vivid,
    /// 0 = greyscale. Clamped to satur8-core's 0..=4 range. Every output shows
    /// this value: it also clears the per-output table below.
    Q_SCRIPTABLE void setSaturation(double saturation);
    /// The base value, which every output without an entry of its own shows.
    Q_SCRIPTABLE double saturation() const;
    /// Blend in linear light (more correct) instead of gamma-encoded sRGB.
    Q_SCRIPTABLE void setLinearLight(bool enabled);
    Q_SCRIPTABLE bool linearLight() const;

    /// One output's value, by the output name `outputs()` lists. A value equal
    /// to the base removes the entry.
    Q_SCRIPTABLE void setOutputSaturation(const QString &output, double saturation);
    /// The whole picture in one call, so no frame shows half of it: the base
    /// value and output name -> value for the outputs that differ.
    Q_SCRIPTABLE void setSaturations(double base, const QVariantMap &outputs);
    /// Output name -> value, for the outputs that differ from the base.
    Q_SCRIPTABLE QVariantMap outputSaturations() const;
    /// Connected output name -> "Manufacturer Model".
    Q_SCRIPTABLE QVariantMap outputs() const;

private:
    void redirectWindow(KWin::EffectWindow *w);
    bool ensureShader();
    /// Switch to a new base and table, redirecting or releasing windows when
    /// the effect turns on or off.
    void setLevels(double base, QHash<QString, double> outputs);
    /// Whether any output shows something other than 1.0.
    bool anyChange() const;
    /// The value for a window: its output's entry, else the base. A window
    /// across two outputs takes the value of the one KWin assigns it to.
    double levelFor(KWin::EffectWindow *w) const;

    std::unique_ptr<KWin::GLShader> m_shader;
    double m_saturation = 1.0;
    QHash<QString, double> m_outputSaturations;
    bool m_linear = false;
    bool m_shaderFailed = false;
};
