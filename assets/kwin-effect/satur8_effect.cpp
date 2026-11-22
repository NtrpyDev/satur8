/*
    Satur8 KWin saturation effect - implementation.

    SPDX-License-Identifier: GPL-3.0-or-later
*/

#include "satur8_effect.h"

#include <core/output.h>
#include <effect/effecthandler.h>
#include <effect/effectwindow.h>
#include <opengl/glshader.h>
#include <opengl/glshadermanager.h>

#include <QDBusConnection>
#include <QDBusVariant>
#include <QStringList>

#include <algorithm>
#include <cmath>
#include <utility>

// Rec.709 luma weights. These MUST match satur8-core's LUMA_* so the look is
// identical to every other backend (DRM CTM, Hyprland, gamescope).
static constexpr float kLumaR = 0.2126f;
static constexpr float kLumaG = 0.7152f;
static constexpr float kLumaB = 0.0722f;

// Single saturation pass. Same math as Saturation::matrix() in satur8-core,
// done per-pixel: out = mix(luma, in, s), which for s > 1 extrapolates away from
// grey (more vivid). KWin textures are premultiplied-alpha, so we divide alpha
// out before mixing and multiply it back afterwards. KWin prepends the right
// #version / precision header for the active GL(ES) profile; we only supply the
// body, matching KWin's own generated MapTexture fragment interface.
//
// When `linearize` is set we do the blend in linear light (sRGB -> linear ->
// mix -> sRGB), which is more physically correct; the default matches
// VibranceGUI's perceptual, gamma-space behaviour so numbers feel familiar.
static const QByteArray kFragment = QByteArrayLiteral(
    "uniform sampler2D sampler;\n"
    "uniform float satur8;\n"
    "uniform vec3 luma;\n"
    "uniform int linearize;\n"
    "in vec2 texcoord0;\n"
    "out vec4 fragColor;\n"
    "\n"
    "vec3 toLinear(vec3 c) {\n"
    "    return mix(c / 12.92, pow((c + 0.055) / 1.055, vec3(2.4)), step(0.04045, c));\n"
    "}\n"
    "vec3 toSrgb(vec3 c) {\n"
    "    return mix(c * 12.92, 1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055, step(0.0031308, c));\n"
    "}\n"
    "\n"
    "void main(void)\n"
    "{\n"
    "    vec4 tex = texture(sampler, texcoord0);\n"
    "    vec3 rgb = (tex.a > 0.0) ? tex.rgb / tex.a : tex.rgb;\n"
    "    if (linearize != 0) {\n"
    "        rgb = toLinear(rgb);\n"
    "        float y = dot(rgb, luma);\n"
    "        rgb = toSrgb(clamp(mix(vec3(y), rgb, satur8), 0.0, 1.0));\n"
    "    } else {\n"
    "        float y = dot(rgb, luma);\n"
    "        rgb = clamp(mix(vec3(y), rgb, satur8), 0.0, 1.0);\n"
    "    }\n"
    "    fragColor = vec4(rgb * tex.a, tex.a);\n"
    "}\n");

static const QString kDBusPath = QStringLiteral("/org/kde/KWin/Effect/Satur81");

Satur8Effect::Satur8Effect()
{
    QDBusConnection::sessionBus().registerObject(
        kDBusPath, this, QDBusConnection::ExportScriptableContents);

    connect(KWin::effects, &KWin::EffectsHandler::windowAdded,
            this, &Satur8Effect::redirectWindow);
}

Satur8Effect::~Satur8Effect()
{
    QDBusConnection::sessionBus().unregisterObject(kDBusPath);
}

bool Satur8Effect::supported()
{
    return KWin::effects->isOpenGLCompositing() && KWin::OffscreenEffect::supported();
}

bool Satur8Effect::ensureShader()
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

void Satur8Effect::redirectWindow(KWin::EffectWindow *w)
{
    if (!w || !anyChange() || !ensureShader()) {
        return;
    }
    redirect(w);
    setShader(w, m_shader.get());
}

bool Satur8Effect::anyChange() const
{
    if (m_saturation != 1.0) {
        return true;
    }
    for (auto it = m_outputSaturations.cbegin(); it != m_outputSaturations.cend(); ++it) {
        if (it.value() != 1.0) {
            return true;
        }
    }
    return false;
}

double Satur8Effect::levelFor(KWin::EffectWindow *w) const
{
    if (m_outputSaturations.isEmpty()) {
        return m_saturation;
    }
    KWin::LogicalOutput *output = w->screen();
    if (!output) {
        return m_saturation;
    }
    return m_outputSaturations.value(output->name(), m_saturation);
}

Satur8DrawResult Satur8Effect::drawWindow(const KWin::RenderTarget &renderTarget,
                                const KWin::RenderViewport &viewport,
                                KWin::EffectWindow *w,
                                int mask,
                                const KWin::Region &deviceRegion,
                                KWin::WindowPaintData &data)
{
    const double level = levelFor(w);
    if (level == 1.0 && !m_outputSaturations.isEmpty()) {
        // This window's output is unchanged while another output is not: draw
        // it the plain way, as OffscreenEffect does for a window it does not
        // redirect.
        return KWin::effects->drawWindow(renderTarget, viewport, w, mask, deviceRegion, data);
    }
    if (m_shader) {
        // We're inside a frame here, so the GL context is current and it is safe
        // to bind the shader and push uniforms. Custom-named uniforms survive the
        // standard uniform setup OffscreenEffect does for the redirected draw.
        KWin::ShaderManager *manager = KWin::ShaderManager::instance();
        manager->pushShader(m_shader.get());
        m_shader->setUniform("satur8", static_cast<float>(level));
        m_shader->setUniform("luma", QVector3D(kLumaR, kLumaG, kLumaB));
        m_shader->setUniform("linearize", m_linear ? 1 : 0);
        manager->popShader();
    }
    // Pass KWin's result through unchanged (bool on 6.8+, void before).
    return KWin::OffscreenEffect::drawWindow(renderTarget, viewport, w, mask, deviceRegion, data);
}

void Satur8Effect::reconfigure(ReconfigureFlags)
{
}

bool Satur8Effect::isActive() const
{
    return m_shader != nullptr && anyChange();
}

void Satur8Effect::setSaturation(double saturation)
{
    setLevels(std::clamp(saturation, 0.0, 4.0), {});
}

void Satur8Effect::setLevels(double base, QHash<QString, double> outputs)
{
    // Entries equal to the base change nothing, so they are not kept.
    for (auto it = outputs.begin(); it != outputs.end();) {
        if (it.value() == base) {
            it = outputs.erase(it);
        } else {
            ++it;
        }
    }
    if (m_saturation == base && m_outputSaturations == outputs) {
        return;
    }

    const bool wasActive = anyChange();
    m_saturation = base;
    m_outputSaturations = std::move(outputs);
    const bool isActive = anyChange();

    if (!wasActive && isActive) {
        const auto windows = KWin::effects->stackingOrder();
        for (KWin::EffectWindow *w : windows) {
            redirectWindow(w);
        }
    } else if (wasActive && !isActive) {
        const auto windows = KWin::effects->stackingOrder();
        for (KWin::EffectWindow *w : windows) {
            unredirect(w);
        }
    }

    KWin::effects->addRepaintFull();
}

double Satur8Effect::saturation() const
{
    return m_saturation;
}

// A D-Bus value for a saturation: finite, clamped to satur8-core's range.
static bool readLevel(QVariant value, double *level)
{
    if (value.userType() == qMetaTypeId<QDBusVariant>()) {
        value = value.value<QDBusVariant>().variant();
    }
    bool ok = false;
    const double read = value.toDouble(&ok);
    if (!ok || !std::isfinite(read)) {
        return false;
    }
    *level = std::clamp(read, 0.0, 4.0);
    return true;
}

void Satur8Effect::setOutputSaturation(const QString &output, double saturation)
{
    if (!std::isfinite(saturation)) {
        return;
    }
    QHash<QString, double> outputs = m_outputSaturations;
    outputs.insert(output, std::clamp(saturation, 0.0, 4.0));
    setLevels(m_saturation, std::move(outputs));
}

void Satur8Effect::setSaturations(double base, const QVariantMap &outputs)
{
    if (!std::isfinite(base)) {
        return;
    }
    QHash<QString, double> table;
    for (auto it = outputs.cbegin(); it != outputs.cend(); ++it) {
        double level = 1.0;
        if (readLevel(it.value(), &level)) {
            table.insert(it.key(), level);
        }
    }
    setLevels(std::clamp(base, 0.0, 4.0), std::move(table));
}

QVariantMap Satur8Effect::outputSaturations() const
{
    QVariantMap result;
    for (auto it = m_outputSaturations.cbegin(); it != m_outputSaturations.cend(); ++it) {
        result.insert(it.key(), it.value());
    }
    return result;
}

QVariantMap Satur8Effect::outputs() const
{
    QVariantMap result;
    const auto screens = KWin::effects->screens();
    for (KWin::LogicalOutput *output : screens) {
        const QString description =
            QStringList{output->manufacturer(), output->model()}.join(QLatin1Char(' ')).trimmed();
        result.insert(output->name(), description);
    }
    return result;
}

void Satur8Effect::setLinearLight(bool enabled)
{
    if (m_linear == enabled) {
        return;
    }
    m_linear = enabled;
    KWin::effects->addRepaintFull();
}

bool Satur8Effect::linearLight() const
{
    return m_linear;
}

KWIN_EFFECT_FACTORY(Satur8Effect, "metadata.json")

#include "satur8_effect.moc"
