// Aurora Veil: grandes faixas luminosas fluindo, gradientes atmosféricos suaves e muito espaço negativo escuro.
// bloom: 0.5 0.55
// param: bands 1 4 3
// param: drift 0 2 1
// param: intensity 0.3 1.5 0.55

// Uma cortina: curva sinuosa de grande amplitude. Devolve o corpo largo (x) e a borda inferior nítida (y).
float2 curtain(float2 p, float t, float seed, float height, float slope, out float d)
{
    float y = height
            + 0.24 * sin(p.x * 0.9 + t + seed)
            + 0.12 * sin(p.x * 2.1 - t * 1.3 + seed * 2.1)
            + 0.16 * (noise(float2(p.x * 0.7 + seed, t * 0.5)) - 0.5) * 2.0
            + slope * p.x;
    d = p.y - y;
    float w = d < 0.0 ? 0.08 : 0.24;
    float body = exp(-pow(d / w, 2.0));
    float edge = exp(-pow((d + 0.02) / 0.04, 2.0));
    return float2(body, edge);
}

float4 ps_main(float4 pos : SV_Position) : SV_Target
{
    float2 uv = frag_uv(pos);
    float2 p = frag_p(pos) + parallax(0.06);
    float t = T() * 0.07 * P(1);

    float3 sky = lerp(float3(0.004, 0.006, 0.02), float3(0.012, 0.025, 0.06), smoothstep(1.0, 0.0, uv.y));
    // névoa tênue embaixo: evita que o espaço negativo vire uma silhueta chapada
    sky += float3(0.0, 0.012, 0.016) * smoothstep(0.55, 0.0, uv.y);
    float3 col = sky;

    float3 green = float3(0.12, 0.80, 0.48);
    float3 teal  = float3(0.05, 0.60, 0.80);
    float3 violet = accent(float3(0.45, 0.25, 0.85));

    int n = (int)round(P(0));
    for (int i = 0; i < 4; i++)
    {
        if (i >= n) break;
        float fi = (float)i;
        float h = -0.34 + 0.34 * fi + 0.04 * sin(t + fi);
        float d;
        float2 c = curtain(p, t + fi * 1.7, fi * 3.3, h, 0.26 - 0.26 * fi, d);

        // Raios finos acompanham a curva e esticam para cima; o corpo largo só é modulado de leve
        float fine = noise(float2(p.x * 38.0 + d * 4.0 + fi * 11.0 + t * 0.6, fi * 3.0));
        float coarse = noise(float2(p.x * 8.0 + d * 2.0 + fi * 9.0 + t * 0.4, fi));
        float rays = 0.45 + 0.45 * coarse + 0.45 * fine * smoothstep(-0.02, 0.15, d);

        float k = saturate(d * 2.4);
        float3 tone = lerp(lerp(green, teal, k), violet, saturate(k * k * 1.2) * 0.8);
        tone = lerp(tone, teal * float3(0.7, 1.0, 1.3), saturate(fi * 0.4));

        float fall = 0.7 / (1.0 + fi * 0.2);
        col += tone * c.x * rays * fall * 1.05 * P(2);
        // borda inferior: fio de luz quase branco-esverdeado, modulado ao longo da curva
        col += lerp(tone, float3(0.85, 1.0, 0.95), 0.5) * c.y * (0.35 + 0.65 * coarse) * fall * 0.45 * P(2);
    }

    col *= 0.45 + 0.55 * vignette(uv, 0.3);
    return float4(finish(col, pos), 1.0);
}
