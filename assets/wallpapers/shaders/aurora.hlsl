// Aurora: cortinas com raios finos sobre um céu estrelado, serra distante e lago que reflete a luz.
// retint: #3ee59a
// bloom: 0.45 0.6
// param: intensity 0.3 1.6 0.6 -- Brilho das cortinas de luz
// param: speed_mul 0 2 1 -- Velocidade do movimento
// param: stars 0 1 0.7 -- Quantidade de estrelas
#define HORIZON 0.24

// Uma cortina: borda inferior nítida e corpo que sobe e se dissolve em raios. `p.y` já é a altura acima do horizonte.
float3 curtain(float2 p, float t, float seed, float base, float amp, float3 low, float3 high)
{
    // dobras lentas: a cortina ondula como tecido, os raios acompanham a dobra
    float fold = 0.55 * sin(p.x * 1.3 + t * 0.8 + seed)
               + 0.28 * sin(p.x * 2.9 - t * 1.1 + seed * 1.7)
               + 0.5 * (fbm(float2(p.x * 0.9 + seed * 3.0, t * 0.35)) - 0.5);
    float edge = base + amp * fold;
    float d = p.y - edge;                          // >0 acima da borda inferior

    // raios: alta frequência em x, esticados na vertical por um leve deslocamento que cresce com a altura
    float sway = p.x + 0.12 * d * sin(p.x * 3.0 + seed) + 0.04 * d * fold;
    float r1 = noise(float2(sway * 34.0 + seed * 7.0, seed + t * 0.6));
    float r2 = noise(float2(sway * 11.0 + seed * 3.0, t * 0.4 + seed));
    float rays = 0.25 + 0.95 * pow(saturate(r1 * 0.65 + r2 * 0.55), 1.6);

    float up = exp(-max(d, 0.0) * 5.2 * (0.9 + 0.6 * r2));   // sobe e se dissolve
    float below = smoothstep(-0.012, 0.0, d);                  // borda inferior firme
    float body = below * up * rays;
    float rim = exp(-pow((d - 0.006) / 0.012, 2.0)) * (0.55 + 0.6 * r2);

    float k = saturate(d * 2.2);
    float3 tone = lerp(low, high, k * k);
    return tone * body + lerp(tone, 1.0.xxx, 0.3) * rim * 0.22;
}

float3 aurora(float2 p, float t)
{
    float3 green = float3(0.10, 0.95, 0.52);
    float3 teal  = float3(0.05, 0.62, 0.80);
    float3 violet = accent(float3(0.55, 0.22, 0.90));
    float3 acc = 0.0.xxx;
    acc += curtain(p, t, 1.0, 0.16, 0.14, green, teal)                    * 1.00;
    acc += curtain(p * float2(0.9, 1.0), t * 0.9, 4.2, 0.30, 0.18, teal, violet) * 0.70;
    acc += curtain(p * float2(1.15, 1.0), t * 1.1, 7.7, 0.08, 0.10, green, green * 0.7 + teal * 0.3) * 0.55;
    return acc;
}

float starfield(float2 p, float t)
{
    float2 g = p * 220.0;
    float2 id = floor(g);
    float2 f = frac(g) - 0.5;
    float h = hash12(id);
    float2 o = (hash22(id) - 0.5) * 0.6;
    float d = length(f - o);
    float tw = 0.65 + 0.35 * sin(t * (1.5 + h * 3.0) + h * 40.0);
    return step(0.955, h) * smoothstep(0.24, 0.0, d) * tw * (0.45 + 0.9 * hash12(id + 7.0));
}

// Serra distante, em duas camadas. Devolve a altura da silhueta acima do horizonte.
float ridge(float x, float scale, float seed)
{
    float n = fbm(float2(x * scale + seed, seed * 1.7));
    float r = 1.0 - abs(2.0 * n - 1.0);
    return lerp(n, r, 0.6);
}

float4 ps_main(float4 pos : SV_Position) : SV_Target
{
    float2 uv = frag_uv(pos);
    float2 p = frag_p(pos) + parallax(0.04);
    float t = T() * 0.06 * P(1);
    float aspect = resolution.x / resolution.y;
    float horizon = HORIZON - 0.5;                 // em coordenadas p (y centrado)
    float h = p.y - horizon;                       // altura acima do horizonte (negativa = lago)

    float3 col;
    if (h >= 0.0)
    {
        // céu: gradiente profundo + luz difusa da aurora perto do horizonte
        col = lerp(float3(0.012, 0.030, 0.050), float3(0.003, 0.005, 0.020), smoothstep(0.0, 0.9, h));
        float3 au = aurora(float2(p.x, h), t) * P(0);
        col += au;
        col += float3(0.0, 0.05, 0.035) * exp(-h * 5.0) * P(0);
        // estrelas só onde a aurora é fraca
        float s = starfield(p, T()) * P(2);
        col += s * (1.0 - saturate(dot(au, 1.0.xxx) * 2.2)) * float3(0.85, 0.92, 1.0) * smoothstep(0.0, 0.12, h);
        // serra: duas camadas, a mais distante é mais clara (névoa)
        float r1 = 0.03 + 0.15 * ridge(p.x, 1.3, 3.0);
        float r2 = 0.008 + 0.09 * ridge(p.x, 2.1, 11.0);
        float aa = 1.5 / resolution.y;
        float m1 = smoothstep(aa, -aa, h - r1);
        float m2 = smoothstep(aa, -aa, h - r2);
        col = lerp(col, float3(0.010, 0.026, 0.040) + au * 0.12, m1 * 0.92);
        col = lerp(col, float3(0.002, 0.006, 0.010) + au * 0.02, m2);
        // luz de borda da aurora no topo da serra
        col += au * exp(-abs(h - r2) * 220.0) * 0.5;
    }
    else
    {
        // lago: reflexo do céu espelhado, rompido por ondas finas e escurecido com a distância
        float depth = -h;
        float ripple = noise(float2(p.x * 22.0, depth * 140.0 - T() * 0.2)) - 0.5 + 0.5 * (noise(float2(p.x * 70.0, depth * 400.0)) - 0.5);
        float2 rp = float2(p.x + ripple * 0.012 * (0.3 + depth * 6.0), depth * 0.5 + 0.02);
        float3 au = aurora(rp, t) * P(0) * 0.85;
        float fade = exp(-depth * 2.2);
        col = lerp(float3(0.012, 0.034, 0.042), float3(0.002, 0.005, 0.012), saturate(depth * 2.2));
        col += float3(0.03, 0.14, 0.11) * exp(-depth * 55.0) * P(0);   // névoa na linha d'água
        col += au * fade;
        col += float3(0.0, 0.16, 0.12) * exp(-depth * 7.0) * P(0) * (0.7 + 0.6 * ripple);
        float r2 = 0.008 + 0.09 * ridge(p.x, 2.1, 11.0);
        float refl = smoothstep(0.0, 0.01, r2 - depth) * smoothstep(0.1, 0.0, depth) * 0.6;
        col = lerp(col, float3(0.002, 0.006, 0.010), refl * 0.8);
    }

    col *= 0.5 + 0.5 * vignette(uv, 0.32);
    return float4(finish(col, pos), 1.0);
}
