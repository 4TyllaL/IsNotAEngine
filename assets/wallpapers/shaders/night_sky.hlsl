// Céu noturno: campo denso de estrelas com cores variadas, Via Láctea com faixas de poeira e estrelas cadentes raras.
// retint: #8fb4ff
// bloom: 0.35 0.7
// param: stars 0.3 1.6 1 -- Quantidade e brilho das estrelas
// param: milky_way 0 1.5 0.9 -- Intensidade da Via Láctea
// param: shooting 0 1 0.6 -- Frequência de estrelas cadentes
// Uma camada de estrelas em grade: cada célula tem no máximo uma estrela, com magnitude, cor e cintilação próprias.
float3 star_layer(float2 p, float scale, float density, float size, float t, float seed)
{
    float2 g = p * scale;
    float2 id = floor(g);
    float2 f = frac(g) - 0.5;
    float3 col = 0.0.xxx;
    for (int j = -1; j <= 1; j++)
    for (int i = -1; i <= 1; i++)
    {
        float2 o = float2(i, j);
        float2 cid = id + o;
        float h = hash12(cid + seed);
        if (h > density) continue;
        float2 c = (hash22(cid + seed * 3.1) - 0.5) * 0.7 + o;
        float d = length(f - c);
        float mag = pow(hash12(cid * 1.7 + seed + 9.0), 3.0);            // poucas brilhantes, muitas fracas
        float temp = hash12(cid + 31.0 + seed);
        float3 tone = lerp(float3(1.0, 0.78, 0.55), float3(0.65, 0.80, 1.0), smoothstep(0.15, 0.85, temp));
        float tw = 0.75 + 0.25 * sin(t * (0.8 + temp * 2.5) + h * 60.0);
        float core = smoothstep(size * (0.5 + mag), 0.0, d);
        float halo = exp(-d * d / (size * size * 9.0)) * mag * 0.35;
        // brilhantes ganham um leve cruzeiro de difração
        float spike = mag > 0.45 ? (exp(-abs(f.x - c.x) * 40.0) * exp(-abs(f.y - c.y) * 4.0) + exp(-abs(f.y - c.y) * 40.0) * exp(-abs(f.x - c.x) * 4.0)) * 0.25 * mag : 0.0;
        col += tone * (core * (0.35 + 1.6 * mag) + halo + spike) * tw;
    }
    return col;
}

float3 milky_way(float2 p)
{
    // eixo da faixa, levemente inclinado
    float2 dir = normalize(float2(1.0, 0.42));
    float2 nrm = float2(-dir.y, dir.x);
    float along = dot(p, dir), across = dot(p, nrm);
    float warp = 0.12 * (fbm(float2(along * 1.4, 2.0)) - 0.5);
    float d = across + warp;
    float band = exp(-d * d / 0.045);
    float core = exp(-d * d / 0.008) * (0.55 + 0.45 * fbm(float2(along * 2.2 + 5.0, 1.0)));
    float cloud = fbm(float2(along * 3.0, d * 6.0) + 7.0);
    float fine = fbm(float2(along * 9.0, d * 16.0) + 20.0);
    // faixas de poeira escura cortando a faixa
    float dust = smoothstep(0.50, 0.68, fbm(float2(along * 4.5 + 3.0, d * 11.0)));
    float dens = (band * (0.35 + 0.9 * cloud) + core) * (1.0 - 0.8 * dust) * (0.7 + 0.5 * fine);
    float3 warm = float3(0.95, 0.80, 0.62), cool = float3(0.45, 0.58, 0.95);
    float3 tone = lerp(cool, warm, saturate(core * 1.3 * (0.4 + cloud)));
    tone = lerp(tone, float3(0.55, 0.42, 0.62), dust * 0.4);
    return tone * dens * 0.17;
}

// Estrela cadente: uma por época, em posição, ângulo e direção sorteados.
float3 shooting_star(float2 p, float t)
{
    const float period = 11.0;
    float epoch = floor(t / period);
    float local = t - epoch * period;
    float life = 1.1;
    if (local > life) return 0.0.xxx;
    float h = hash12(float2(epoch, 3.7));
    if (h > 0.8) return 0.0.xxx;                                    // nem toda época tem uma
    float2 start = (hash22(float2(epoch, 1.3)) - 0.5) * float2(1.4, 0.5) + float2(0.0, 0.18);
    float ang = 3.6 + hash12(float2(epoch, 8.1)) * 0.9;
    float2 dir = float2(cos(ang), sin(ang));
    float k = local / life;
    float2 head = start + dir * (k * 0.55);
    float2 q = p - head;
    float along = dot(q, -dir);                                     // >0 atrás da cabeça (cauda)
    float perp = abs(dot(q, float2(-dir.y, dir.x)));
    float tail = along > 0.0 ? exp(-along * 14.0) * exp(-perp * perp / 0.00002) * smoothstep(0.0, 0.01, along) : 0.0;
    float headg = exp(-dot(q, q) / 0.00005);
    float env = sin(k * 3.14159);
    return float3(0.9, 0.95, 1.0) * (tail * 2.4 + headg * 2.5) * env;
}

float4 ps_main(float4 pos : SV_Position) : SV_Target
{
    float2 uv = frag_uv(pos);
    float2 p = frag_p(pos) + parallax(0.015);
    float t = T();

    // rotação lenta do céu em torno de um polo fora da tela
    float2 pole = float2(0.55, -0.9);
    float a = t * 0.0025;
    float2 q = mul(rot(a), p - pole) + pole;

    // fundo: azul-noite com leve brilho atmosférico perto da borda inferior
    float3 col = lerp(float3(0.012, 0.018, 0.040), float3(0.003, 0.005, 0.016), smoothstep(-0.5, 0.6, p.y));
    col += float3(0.012, 0.022, 0.030) * smoothstep(0.1, -0.55, p.y);

    float3 mw = milky_way(q) * P(1);
    col += mw;

    float3 st = star_layer(q, 14.0, 0.42, 0.05, t, 1.0) * 0.5
              + star_layer(q, 32.0, 0.34, 0.07, t, 2.0) * 0.55
              + star_layer(q, 78.0, 0.27, 0.12, t, 3.0) * 0.7;
    // mais estrelas dentro da Via Láctea
    float mwd = saturate(dot(mw, 1.0.xxx) * 4.0);
    st += star_layer(q, 120.0, 0.30 * mwd, 0.14, t, 4.0) * 0.7;
    col += st * P(0);

    col += shooting_star(p, t) * P(2);

    col *= 0.55 + 0.45 * vignette(uv, 0.3);
    return float4(finish(col, pos), 1.0);
}
