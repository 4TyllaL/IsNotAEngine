// Fumaça: domain warping lento com iluminação direcional e grão de filme.
// param: flow 0 2 1 -- Velocidade da fumaça
// param: bright 0.5 1.6 1 -- Brilho da fumaça
// param: warmth 0 0.6 0.25 -- Calor da luz alaranjada
// param: density 0.6 1.5 1 -- Densidade (mais encorpada ou mais rala)

float pattern(float2 p, out float2 q, out float2 r)
{
    float t = T() * 0.035 * P(0);
    q = float2(fbm(p + float2(0.0, t)), fbm(p + float2(5.2, 1.3 - t)));
    r = float2(fbm(p + 3.5 * q + float2(1.7, 9.2) + t * 1.5), fbm(p + 3.5 * q + float2(8.3, 2.8) - t));
    return fbm(p + 3.0 * r);
}

float4 ps_main(float4 pos : SV_Position) : SV_Target
{
    float2 uv = frag_uv(pos);
    float2 p = cursor_swirl(frag_p(pos), 0.3, 1.4) * 1.6 + parallax(0.15);

    float2 q, r;
    float f = pattern(p, q, r);
    // Normal aproximada para luz lateral
    float e = 0.01;
    float2 q2, r2;
    float fx = pattern(p + float2(e, 0.0), q2, r2);
    float fy = pattern(p + float2(0.0, e), q2, r2);
    float3 n = normalize(float3((f - fx) / e, (f - fy) / e, 12.0));
    float light = saturate(dot(n, normalize(float3(-0.6, 0.5, 0.6))));

    float3 base = float3(0.02, 0.022, 0.03);
    float3 smoke = accent(float3(0.62, 0.66, 0.75));
    float dens = smoothstep(0.35, 0.95, f * 1.2 * P(3) + length(r) * 0.25);
    float wisps = pow(saturate(f * 1.5 * P(3) - 0.35), 2.5);
    float3 col = base + smoke * wisps * (0.55 + 0.45 * light) * 0.9;
    col += float3(0.25, 0.18, 0.12) * pow(dens, 3.0) * P(2) * (1.0 - uv.x);

    col = aces(col * 1.3 * P(1));
    col *= 0.55 + 0.45 * vignette(uv, 0.35);
    return float4(finish(col, pos), 1.0);
}
