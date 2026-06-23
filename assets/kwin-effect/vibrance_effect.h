/*
    Vibrance - a single-purpose KWin saturation effect.

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

#include <QVector3D>
#include <memory>

namespace KWin
{
class GLShader;
class EffectWindow;
}

class VibranceEffect : public KWin::OffscreenEffect
{
    Q_OBJECT
    Q_CLASSINFO("D-Bus Interface", "org.kde.kwin.Effect.Vibrance")

public:
    VibranceEffect();
    ~VibranceEffect() override;

    static bool supported();

    void reconfigure(ReconfigureFlags flags) override;
    int requestedEffectChainPosition() const override
    {
        return 99; // late: act on the final, composited look
    }
    bool isActive() const override;

protected:
    void drawWindow(const KWin::RenderTarget &renderTarget,
                    const KWin::RenderViewport &viewport,
                    KWin::EffectWindow *w,
                    int mask,
                    const KWin::Region &deviceRegion,
                    KWin::WindowPaintData &data) override;

public Q_SLOTS:
    /// Live saturation control over D-Bus. 1.0 = unchanged, >1 = more vivid,
    /// 0 = greyscale. Clamped to vibrance-core's 0..=4 range.
    Q_SCRIPTABLE void setSaturation(double saturation);
    Q_SCRIPTABLE double saturation() const;

private:
    void redirectWindow(KWin::EffectWindow *w);
    bool ensureShader();

    std::unique_ptr<KWin::GLShader> m_shader;
    double m_saturation = 1.0;
    bool m_shaderFailed = false;
};
