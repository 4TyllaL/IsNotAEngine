// Modelo: ondas. Linhas que ondulam sobre um fundo escuro; edite o código ou use os controles.
// param: ondas 3 24 10 -- Quantas linhas de onda
// param: amplitude 0.02 0.4 0.12 -- Altura das ondas
// param: velocidade 0 2 1 -- Velocidade da animação
// param: espessura 0.002 0.03 0.008 -- Espessura de cada linha
// param: brilho 0.3 2 1 -- Brilho geral

float4 ps_main(float4 pos : SV_Position) : SV_Target
{
    float2 uv = frag_uv(pos);
    float2 p = frag_p(pos);
    float t = T() * 0.4 * P(2);
    float3 base = lerp(float3(0.03, 0.04, 0.09), float3(0.10, 0.05, 0.16), uv.y);
    float3 col = base;
    float3 cor_a = accent(float3(0.30, 0.65, 1.00));
    float3 cor_b = float3(1.00, 0.35, 0.70);
    int n = (int)round(P(0));
    for (int i = 0; i < 24; i++)
    {
        if (i >= n) break;
        float k = n > 1 ? (float)i / (float)(n - 1) : 0.5;
        float y0 = lerp(-0.45, 0.45, k);
        float onda = sin(p.x * (3.0 + k * 2.0) + t * (1.0 + k) + k * 6.0) * P(1)
                   + sin(p.x * 7.0 - t * 1.7 + k * 3.0) * P(1) * 0.3;
        float d = abs(p.y - (y0 + onda * (0.4 + 0.6 * k)));
        float linha = smoothstep(P(3), 0.0, d);
        float halo = exp(-d * 40.0) * 0.25;
        col += lerp(cor_a, cor_b, k) * (linha + halo) * P(4);
    }
    col *= 0.55 + 0.45 * vignette(uv, 0.35);
    return float4(finish(col, pos), 1.0);
}
