// Gyroid · Seda: voo por dentro de uma superfície giroide (sin(x)cos(y) + sin(y)cos(z) + sin(z)cos(x)),
// com brilho neon acumulado perto das paredes e cores que escorrem devagar como seda.
// bloom: 0
// param: flow 0 2 1 -- Velocidade do voo
// param: glow 0.4 1.8 1 -- Brilho neon nas paredes
// param: hue 0 1 0 -- Deslocamento das cores
// param: fog 0.4 2 1 -- Névoa (quanto a seda some ao longe)

float3 palette(float t)
{
    return 0.5 + 0.5 * cos(TAU * (float3(0.00, 0.33, 0.67) + t));
}

float gyroid(float3 p)
{
    return dot(sin(p), cos(p.yzx));
}

float map(float3 p)
{
    float g = gyroid(p * 2.7) * 0.62 + gyroid(p * 6.1) * 0.16;
    return (abs(g) - 0.16) * 0.45;
}

float4 ps_main(float4 pos : SV_Position) : SV_Target
{
    float2 uv = frag_p(pos);
    float t = T() * 0.30 * P(0);
    float2 par = parallax(0.25);

    // câmera: avança em z e balança de leve
    float3 ro = float3(0.9 + sin(t * 0.9) * 0.35, 0.9 + cos(t * 0.7) * 0.30, t * 2.4);
    float3 fw = normalize(float3(sin(t * 0.5) * 0.25 + par.x, cos(t * 0.6) * 0.20 + par.y, 1.0));
    float3 rt = normalize(cross(float3(0.0, 1.0, 0.0), fw));
    float3 up = cross(fw, rt);
    float roll = sin(t * 0.4) * 0.25;
    uv = mul(uv, float2x2(cos(roll), -sin(roll), sin(roll), cos(roll)));
    float3 rd = normalize(fw + uv.x * rt + uv.y * up);

    // O começo do raio varia de pixel para pixel: o número de passos (inteiro) deixaria faixas de cor no degradê,
    // e assim elas viram um ruído fino que o olho não separa.
    float d = hash12(pos.xy) * 0.06;
    float glow = 0.0;
    float steps = 0.0;
    for (int i = 0; i < 80; i++)
    {
        float h = map(ro + rd * d);
        glow += 0.011 / (0.02 + abs(h));
        if (h < 0.0015 || d > 16.0) break;
        d += h;
        steps += 1.0;
    }

    float fog = exp(-d * 0.11 * P(3));
    float hue = P(2);
    float3 col = palette(d * 0.05 + t * 0.25 + hue) * glow * 0.10 * P(1) * fog;
    col += palette(t * 0.15 + 0.5 + hue) * (steps / 80.0) * 0.6 * fog;
    col = col / (1.0 + col);                             // comprime os realces
    col = pow(max(col, 0.0), 0.8);
    col *= 1.0 - 0.55 * dot(uv, uv);                     // vinheta
    return float4(finish(col, pos), 1.0);                // grão e dither: sem as faixas de cor do degradê
}
