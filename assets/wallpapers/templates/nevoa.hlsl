// Modelo: névoa de cor. Duas cores que se misturam devagar em uma névoa suave.
// param: escala 0.5 4 1.6 -- Tamanho das nuvens (maior = mais detalhe)
// param: velocidade 0 2 1 -- Velocidade da animação
// param: contraste 0.5 2.5 1.2 -- Contraste entre claro e escuro
// param: mistura 0 1 0.5 -- Quanto a segunda cor aparece
// param: brilho 0.3 1.6 1 -- Brilho geral

float4 ps_main(float4 pos : SV_Position) : SV_Target
{
    float2 uv = frag_uv(pos);
    float2 p = frag_p(pos) * P(0) + parallax(0.1);
    float t = T() * 0.05 * P(1);
    float2 q = float2(fbm(p + t), fbm(p + float2(5.2, 1.3) - t));
    float f = fbm(p + 2.5 * q + float2(1.7, 9.2) + t * 0.7);
    f = saturate((f - 0.5) * P(2) + 0.5);
    float3 cor_a = accent(float3(0.10, 0.22, 0.55));
    float3 cor_b = float3(0.85, 0.30, 0.55);
    float3 col = lerp(float3(0.015, 0.02, 0.05), cor_a, smoothstep(0.1, 0.7, f));
    col = lerp(col, cor_b, smoothstep(0.55, 0.95, f) * P(3));
    col *= P(4) * (0.5 + 0.5 * vignette(uv, 0.3));
    return float4(finish(col, pos), 1.0);
}
