// Topography Cobalt: relevo orgânico com curvas de nível finas, fundo azul-marinho e realces cobalto.
// bloom: 0.35 0.7
// param: density 8 60 34
// param: warp 0 1.5 0.7
// param: glow 0 1 0.5

float height(float2 p)
{
    float t = T() * 0.003;
    // Domain warp lento e suave (ruído de baixa frequência): curvas fluidas, sem serrilhado
    float2 q = float2(noise(p * 0.45 + float2(t, 0.0)), noise(p * 0.45 + float2(5.2, 1.3 - t)));
    p += (q - 0.5) * P(1) * 2.4;
    return 0.58 * noise(p * 0.55 + t * 2.0) + 0.30 * noise(p * 1.1 - t) + 0.12 * noise(p * 2.3 + t);
}

float4 ps_main(float4 pos : SV_Position) : SV_Target
{
    float2 uv = frag_uv(pos);
    float2 p = frag_p(pos) * 2.2 + parallax(0.1);
    float h = height(p);

    float v = h * P(0);
    // Iluminação por gradiente (relevo sutil sob as linhas)
    float2 e = float2(0.01, 0.0);
    float2 g = float2(height(p + e.xy) - height(p - e.xy), height(p + e.yx) - height(p - e.yx)) / 0.02;
    float shade = saturate(0.5 + dot(normalize(g + 1e-4), float2(-0.6, 0.8)) * saturate(length(g) * 0.5) * 0.5);

    float3 navy_lo = float3(0.006, 0.018, 0.06);
    float3 navy_hi = float3(0.018, 0.05, 0.15);
    float3 cobalt = accent(float3(0.10, 0.34, 1.0));

    float3 col = lerp(navy_lo, navy_hi, smoothstep(0.15, 0.85, h)) * (0.7 + 0.6 * shade);

    // Duas famílias de curvas: finas e mestras (a cada 5)
    float dm = min(frac(v), 1.0 - frac(v));
    float minor = 1.0 - smoothstep(0.0, 1.1 * fwidth(v) + 1e-4, dm);
    // Nos extremos locais o gradiente zera e a linha degenera em um ponto: apaga ali
    float flat = smoothstep(0.0003, 0.003, fwidth(v));
    minor *= flat;
    float vm = v / 5.0;
    float major = 1.0 - smoothstep(0.0, 1.8 * fwidth(vm) + 1e-4, min(frac(vm), 1.0 - frac(vm)));

    major *= flat;

    // Linhas se acendem com a altitude; vales ficam quase apagados
    float alt = smoothstep(0.2, 0.9, h);
    col += cobalt * minor * (0.22 + 0.45 * alt);
    col += lerp(cobalt, float3(0.7, 0.85, 1.0), 0.35) * major * (0.35 + 0.5 * alt);

    // Brilho suave nos cumes
    col += cobalt * pow(alt, 3.0) * P(2) * 0.18;

    // Onda lenta de luz que varre as curvas
    float sweep = pow(0.5 + 0.5 * sin(h * 9.0 - T() * 0.08 + p.x * 0.3), 10.0);
    col += cobalt * (minor + major) * sweep * 0.4 * P(2);

    col *= 0.45 + 0.55 * vignette(uv, 0.35);
    return float4(finish(col, pos), 1.0);
}
