// Topografia escura: curvas de nível finas sobre relevo que evolui devagar, com sombreamento suave e marcas de levantamento.
// retint: #5fa8c8
// bloom: 0.25 0.7
// param: density 6 40 22 -- Quantidade de curvas de nível
// param: flow 0 2 1 -- Velocidade da evolução do relevo
// param: relief 0 1 0.55

float height(float2 p, float z)
{
    float2 w = float2(fbm3(float3(p * 0.9, z)), fbm3(float3(p * 0.9 + 17.0, z)));
    float h = fbm3(float3(p * 0.8 + (w - 0.5) * 1.1, z * 1.3));
    // realça cristas e vales com um leve afunilamento: o relevo fica menos "nuvem" e mais "terreno"
    h = (h - 0.3) * 2.2 + 0.1;                       // fbm fica concentrado no meio: reabre a faixa de cotas
    return h * 0.78 + 0.22 * smoothstep(0.35, 0.75, saturate(h));
}

// Linha de contorno anti-aliased: 1 no fio, 0 fora, largura constante em pixels.
float contour(float h, float levels, float px)
{
    float v = h * levels;
    float f = abs(frac(v - 0.5) - 0.5);
    float w = max(fwidth(v), 1e-4) * px;
    return 1.0 - smoothstep(0.0, w, f);
}

float4 ps_main(float4 pos : SV_Position) : SV_Target
{
    float2 uv = frag_uv(pos);
    float2 p = frag_p(pos) * 1.6 + parallax(0.04) * 1.6;
    float z = T() * 0.012 * P(1);

    float h = height(p, z);
    float e = 0.004;
    float dx = height(p + float2(e, 0), z) - h;
    float dy = height(p + float2(0, e), z) - h;
    float3 n = normalize(float3(-dx / e * P(2) * 0.6, -dy / e * P(2) * 0.6, 1.0));
    float shade = saturate(dot(n, normalize(float3(-0.5, 0.6, 0.65))));

    float levels = P(0);
    float minor = contour(h, levels, 0.9);
    float major = contour(h, levels / 5.0, 1.6);

    float3 deep = float3(0.006, 0.009, 0.016);
    float3 mid = float3(0.020, 0.034, 0.050);
    float3 col = lerp(deep, mid, smoothstep(0.2, 0.8, h));
    col += float3(0.012, 0.022, 0.030) * shade * (0.5 + 0.5 * P(2));

    float3 line_c = accent(float3(0.38, 0.66, 0.78));
    // linhas ficam mais claras nas cotas altas; as de índice são mais firmes
    float lvl = 0.35 + 0.65 * smoothstep(0.1, 0.9, h);
    col += line_c * minor * 0.36 * lvl;
    col += lerp(line_c, 1.0.xxx, 0.35) * major * 0.85 * lvl;

    // marcas de levantamento: cruz de registro nos cantos e pequenas cotas ao longo da borda inferior
    float2 px = pos.xy;
    float2 res = resolution;
    float m = 0.0;
    float2 corners[4] = { float2(48, 48), float2(res.x - 48, 48), float2(48, res.y - 48), float2(res.x - 48, res.y - 48) };
    for (int i = 0; i < 4; i++)
    {
        float2 d = abs(px - corners[i]);
        m += (d.x < 9.0 && d.y < 0.6) || (d.y < 9.0 && d.x < 0.6) ? 1.0 : 0.0;
    }
    float tick = (abs(frac((px.x - 48.0) / 96.0 + 0.5) - 0.5) < 0.5 / 96.0 * 1.2 && px.y > res.y - 48.0 && px.y < res.y - 40.0 && px.x > 48.0 && px.x < res.x - 48.0) ? 1.0 : 0.0;
    col += line_c * (m * 0.5 + tick * 0.25);

    col *= 0.45 + 0.55 * vignette(uv, 0.3);
    return float4(finish(col, pos), 1.0);
}
