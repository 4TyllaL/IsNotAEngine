// Fallback interno: fundo escuro e discreto usado quando um wallpaper falha ao carregar.
// Não faz parte do catálogo (não aparece no Studio) e é bem leve para a GPU.
float4 ps_main(float4 pos : SV_Position) : SV_Target
{
    float2 uv = frag_uv(pos);
    float2 p = frag_p(pos);
    float lift = smoothstep(-0.6, 0.8, p.y + 0.15 * sin(p.x * 1.5 + T() * 0.05));
    float3 col = lerp(float3(0.012, 0.016, 0.030), float3(0.030, 0.040, 0.070), lift);
    col += float3(0.020, 0.030, 0.060) * exp(-dot(p, p) * 1.6);
    col *= 0.6 + 0.4 * vignette(uv, 0.3);
    return float4(finish(col, pos), 1.0);
}
