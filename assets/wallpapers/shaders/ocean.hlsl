// Oceano: marés de névoa iluminadas por trás, com sombra própria e filamentos finos que correm no fluxo.
// bloom: 0.35 0.7
// param: density 0.4 1.6 1 -- Densidade da névoa
// param: flow 0 2 1 -- Velocidade do fluxo
// param: glow 0 1.5 1 -- Brilho da luz por trás
float pattern(float2 p, float t, out float2 r)
{
    float2 q = float2(fbm(p + float2(0.0, t)), fbm(p + float2(5.2, 1.3 - t)));
    r = float2(fbm(p + 3.2 * q + float2(1.7, 9.2) + t * 1.4), fbm(p + 3.2 * q + float2(8.3, 2.8) - t));
    return fbm(p + 2.6 * r);
}

float4 ps_main(float4 pos : SV_Position) : SV_Target
{
    float2 uv = frag_uv(pos);
    float2 p = cursor_swirl(frag_p(pos), 0.3, 1.2) * 1.5 + parallax(0.12);
    float t = T() * 0.03 * P(1);

    float2 r;
    float f = pattern(p, t, r);
    float dens = smoothstep(0.30, 0.82, f * 1.15 + length(r) * 0.22) * P(0);

    // luz atrás da fumaça, no alto à direita: a fumaça fina deixa passar, a densa fica escura e com borda clara
    float2 lp = float2(0.55, 0.25) + 0.05 * float2(sin(T() * 0.05), cos(T() * 0.04));
    float2 toL = lp - p;
    float glow = exp(-dot(toL, toL) * 1.5) * P(2);
    // auto-sombra: densidade um pouco adiante, em direção à luz
    float2 r2;
    float fl = pattern(p + normalize(toL) * 0.10, t, r2);
    float shade = exp(-smoothstep(0.30, 0.82, fl * 1.15 + length(r2) * 0.22) * 1.6);

    // filamentos finos que correm ao longo do fluxo
    float rid = 1.0 - abs(2.0 * noise(p * 5.5 + r * 3.5 + t * 2.0) - 1.0);
    float fil = pow(saturate(rid), 3.0) * dens;

    float3 cool = accent(float3(0.55, 0.62, 0.78));
    float3 warm = float3(1.0, 0.74, 0.52);
    float3 lightc = lerp(cool, warm, saturate(glow * 0.8));

    float3 col = float3(0.010, 0.012, 0.020) + cool * 0.015;
    col += lightc * dens * (0.05 + 1.35 * glow * shade) * (0.85 + 0.25 * fil);
    col += lightc * pow(saturate(dens), 2.0) * (1.0 - shade) * 0.05;                    // fumaça densa na sombra, bem escura
    col += warm * glow * 0.10 * (1.0 - dens * 0.7);                                      // halo da luz através da névoa fina

    col = aces(col * 1.15);
    col *= 0.55 + 0.45 * vignette(uv, 0.33);
    return float4(finish(col, pos), 1.0);
}
