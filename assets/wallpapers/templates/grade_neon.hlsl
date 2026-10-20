// Modelo: grade neon. Um chão em perspectiva que avança em direção ao horizonte.
// param: densidade 4 24 10 -- Quantidade de linhas da grade
// param: velocidade 0 3 1 -- Velocidade com que a grade avança
// param: brilho 0.2 2 1 -- Brilho das linhas
// param: horizonte 0.2 0.7 0.45 -- Altura do horizonte na tela
// param: sol 0 1 0.6 -- Intensidade do sol no horizonte

float4 ps_main(float4 pos : SV_Position) : SV_Target
{
    float2 uv = frag_uv(pos);
    float2 p = frag_p(pos);
    float hz = P(3) - 0.5;
    float3 ceu = lerp(float3(0.02, 0.01, 0.07), float3(0.30, 0.06, 0.34), smoothstep(0.55, -0.1, p.y - hz));
    float3 col = ceu;

    // sol atrás do horizonte
    float2 sp = p - float2(0.0, hz + 0.16);
    float disco = smoothstep(0.22, 0.20, length(sp));
    float3 cor_sol = lerp(float3(1.0, 0.25, 0.45), float3(1.0, 0.85, 0.35), saturate(sp.y * 3.0 + 0.5));
    col = lerp(col, cor_sol, disco * P(4));

    // chão em perspectiva
    float dy = hz - p.y;
    if (dy > 0.0)
    {
        float z = 0.35 / dy;
        float x = p.x * z;
        float zz = z + T() * P(1) * 0.6;
        float dens = P(0) * 0.25;
        float gx = abs(frac(x * dens) - 0.5);
        float gz = abs(frac(zz * dens) - 0.5);
        float largura = 0.02 + 0.015 * z;
        float linha = max(smoothstep(largura, 0.0, gx), smoothstep(largura, 0.0, gz));
        float fade = exp(-z * 0.14);
        float3 neon = accent(float3(0.25, 0.85, 1.0));
        col = lerp(float3(0.03, 0.01, 0.08), col, 0.0) + neon * linha * fade * P(2) * 1.6;
        col += float3(0.5, 0.1, 0.6) * exp(-dy * 18.0) * 0.5 * P(2);
    }
    col *= 0.6 + 0.4 * vignette(uv, 0.3);
    return float4(finish(col, pos), 1.0);
}
