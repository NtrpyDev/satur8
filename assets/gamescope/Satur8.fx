// Satur8 saturation effect for gamescope's ReShade path (B6 fallback).
//
// Self-contained (gamescope does not ship ReShade.fxh): declares the backbuffer
// sampler and a full-screen-triangle vertex shader inline. This shipped copy
// uses a default saturation; `satur8 run --via gamescope` regenerates it in
// ~/.local/share/gamescope/reshade/Shaders/ with the requested value baked in.
uniform float Saturation = 1.4;

texture ColorTex : COLOR;
sampler Back { Texture = ColorTex; };

void VS_Tri(in uint id : SV_VertexID, out float4 pos : SV_Position, out float2 uv : TEXCOORD)
{
    uv  = float2((id << 1) & 2, id & 2);
    pos = float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
}

float4 PS_Satur8(float4 pos : SV_Position, float2 uv : TEXCOORD) : SV_Target
{
    float3 c = tex2D(Back, uv).rgb;
    float luma = dot(c, float3(0.2126, 0.7152, 0.0722));
    return float4(lerp(luma.xxx, c, Saturation), 1.0);
}

technique Satur8
{
    pass { VertexShader = VS_Tri; PixelShader = PS_Satur8; }
}
