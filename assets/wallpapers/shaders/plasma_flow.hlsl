// Plasma / Chromatic Flow: campos de cor suaves que se interpenetram, paleta contida e movimento em camadas.
// bloom: 0.3 0.7
// param: scale 0.5 3 1 -- Tamanho dos campos de cor
// param: flow 0 2 1 -- Velocidade do movimento
// param: sheen 0 1 0.5 -- Brilho acetinado
// Campo escalar fluido: duas camadas de warp suave (sem cantos)
float field(float2 p, float t, float seed)
{
    float2 q = float2(noise(p * 0.7 + float2(t, seed)), noise(p * 0.7 + float2(seed * 2.0, -t)));
    float2 r = float2(noise(p * 0.9 + q * 1.4 + float2(t * 0.7, 3.1)), noise(p * 0.9 + q * 1.4 + float2(-t * 0.5, 7.7)));
    return 0.65 * noise(p * 0.6 + r * 1.6 + seed) + 0.35 * noise(p * 1.2 + q * 1.1 - seed);
}

// Rampa de cor contínua: base profunda -> azul-índigo -> magenta -> pêssego (acento raro)
float3 ramp(float x)
{
    float3 c0 = float3(0.022, 0.018, 0.10);
    float3 c1 = float3(0.10, 0.08, 0.40);
    float3 c2 = accent(float3(0.62, 0.17, 0.52));
    float3 c3 = float3(1.00, 0.62, 0.52);
    float3 c = lerp(c0, c1, smoothstep(0.15, 0.45, x));
    c = lerp(c, c2, smoothstep(0.42, 0.72, x));
    return lerp(c, c3, smoothstep(0.82, 1.02, x) * 0.7);
}

float4 ps_main(float4 pos : SV_Position) : SV_Target
{
    float2 uv = frag_uv(pos);
    float2 p = frag_p(pos) * 2.0 * P(0) + parallax(0.2);
    float t = T() * 0.012 * P(1);

    float f = field(p, t, 0.0);
    float g = field(p * 0.8 + 6.0, t * 0.8, 11.0);

    // Dois campos se combinam: g desloca a rampa, criando bandas que se cruzam
    float x = saturate((f - 0.30) * 1.75 + (g - 0.5) * 0.6);
    float3 col = ramp(x);

    // Brilho de seda: normal a partir do gradiente do campo
    float2 e = float2(0.02, 0.0);
    float2 gr = float2(field(p + e.xy, t, 0.0) - field(p - e.xy, t, 0.0),
                       field(p + e.yx, t, 0.0) - field(p - e.yx, t, 0.0)) / 0.04;
    // dobras finas de seda sobre o relevo largo
    float2 fine = float2(noise(p * 4.1 + gr * 2.0 + t * 3.0), noise(p * 4.1 + 9.0 - gr * 2.0 - t * 3.0)) - 0.5;
    float3 n = normalize(float3(-gr * 0.9 + fine * 0.9, 1.0));
    float3 h = normalize(float3(0.35, 0.5, 0.8));
    float spec = pow(saturate(dot(n, h)), 40.0);
    col += lerp(col, float3(1.0, 0.8, 0.9), 0.4) * spec * smoothstep(0.25, 0.55, x) * P(2) * 0.55;
    col *= 0.8 + 0.4 * saturate(n.z - 0.6);

    col *= 0.5 + 0.5 * vignette(uv, 0.35);
    return float4(finish(col, pos), 1.0);
}
