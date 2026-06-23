/*
    Vibrance KWin saturation effect - implementation.

    SPDX-License-Identifier: GPL-3.0-or-later
*/

#include "vibrance_effect.h"

#include <effect/effecthandler.h>
#include <opengl/glshader.h>
#include <opengl/glshadermanager.h>

#include <QDBusConnection>

#include <algorithm>

// Rec.709 luma weights. These MUST match vibrance-core's LUMA_* so the look is
// identical to every other backend (DRM CTM, Hyprland, gamescope).
static constexpr float kLumaR = 0.2126f;
static constexpr float kLumaG = 0.7152f;
static constexpr float kLumaB = 0.0722f;

// Single saturation pass. Same math as Saturation::matrix() in vibrance-core,
// done per-pixel: out = mix(luma, in, s), which for s > 1 extrapolates away from
// grey (more vivid). KWin textures are premultiplied-alpha, so we divide alpha
// out before mixing and multiply it back afterwards. KWin prepends the right
// #version / precision header for the active GL(ES) profile; we only supply the
// body, matching KWin's own generated MapTexture fragment interface.
static const QByteArray kFragment = QByteArrayLiteral(
    "uniform sampler2D sampler;\n"
    "uniform float vibrance;\n"
    "uniform vec3 luma;\n"
    "in vec2 texcoord0;\n"
    "out vec4 fragColor;\n"
    "\n"
    "void main(void)\n"
    "{\n"
    "    vec4 tex = texture(sampler, texcoord0);\n"
    "    vec3 rgb = (tex.a > 0.0) ? tex.rgb / tex.a : tex.rgb;\n"
    "    float y = dot(rgb, luma);\n"
    "    rgb = clamp(mix(vec3(y), rgb, vibrance), 0.0, 1.0);\n"
    "    fragColor = vec4(rgb * tex.a, tex.a);\n"
    "}\n");

static const QString kDBusPath = QStringLiteral("/org/kde/KWin/Effect/Vibrance1");

VibranceEffect::VibranceEffect()
{
    QDBusConnection::sessionBus().registerObject(
        kDBusPath, this, QDBusConnection::ExportScriptableContents);

    connect(KWin::effects, &KWin::EffectsHandler::windowAdded,
            this, &VibranceEffect::redirectWindow);
    const auto windows = KWin::effects->stackingOrder();
    for (KWin::EffectWindow *w : windows) {
        redirectWindow(w);
    }
}

VibranceEffect::~VibranceEffect()
{
    QDBusConnection::sessionBus().unregisterObject(kDBusPath);
}

bool VibranceEffect::supported()
{
    return KWin::effects->isOpenGLCompositing() && KWin::OffscreenEffect::supported();
}

bool VibranceEffect::ensureShader()
{
    if (m_shader) {
        return true;
    }
    if (m_shaderFailed) {
        return false;
    }
    m_shader = KWin::ShaderManager::instance()->generateCustomShader(
        KWin::ShaderTrait::MapTexture, QByteArray(), kFragment);
    if (!m_shader) {
        m_shaderFailed = true;
        return false;
    }
    return true;
}

void VibranceEffect::redirectWindow(KWin::EffectWindow *w)
{
    if (!w || !ensureShader()) {
        return;
    }
    redirect(w);
    setShader(w, m_shader.get());
}

void VibranceEffect::drawWindow(const KWin::RenderTarget &renderTarget,
                                const KWin::RenderViewport &viewport,
                                KWin::EffectWindow *w,
                                int mask,
                                const KWin::Region &deviceRegion,
                                KWin::WindowPaintData &data)
{
    if (m_shader && m_saturation != 1.0) {
        // We're inside a frame here, so the GL context is current and it is safe
        // to bind the shader and push uniforms. Custom-named uniforms survive the
        // standard uniform setup OffscreenEffect does for the redirected draw.
        KWin::ShaderManager *manager = KWin::ShaderManager::instance();
        manager->pushShader(m_shader.get());
        m_shader->setUniform("vibrance", static_cast<float>(m_saturation));
        m_shader->setUniform("luma", QVector3D(kLumaR, kLumaG, kLumaB));
        manager->popShader();
    }
    KWin::OffscreenEffect::drawWindow(renderTarget, viewport, w, mask, deviceRegion, data);
}

void VibranceEffect::reconfigure(ReconfigureFlags)
{
}

bool VibranceEffect::isActive() const
{
    return m_shader != nullptr;
}

void VibranceEffect::setSaturation(double saturation)
{
    saturation = std::clamp(saturation, 0.0, 4.0);
    if (m_saturation == saturation) {
        return;
    }
    m_saturation = saturation;
    KWin::effects->addRepaintFull();
}

double VibranceEffect::saturation() const
{
    return m_saturation;
}

KWIN_EFFECT_FACTORY(VibranceEffect, "metadata.json")

#include "vibrance_effect.moc"
