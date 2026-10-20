// Liquid Chrome: superfície metálica ondulada refletindo um estúdio com softboxes.
// retint: #5a8cff
// bloom: 0.25 0.75
// param: flow 0 2 1 -- Velocidade do fluxo do metal
// param: shine 0.6 1.5 1 -- Intensidade dos reflexos
float height_at(float2 p)
{
    float t = T() * 0.12 * P(0);
    float h = 0.0;
    h += sin(p.x * 2.3 + t * 1.3 + sin(p.y * 1.7 + t)) * 0.5;
    h += sin(p.y * 2.9 - t * 1.1 + sin(p.x * 2.1 - t * 0.7)) * 0.4;
    h += sin((p.x + p.y) * 4.1 + t * 1.7) * 0.15;
    h += sin(p.x * 7.3 - p.y * 5.1 + t * 2.1) * 0.035;
    return h;
}

// Faixa de luz com borda firme (softbox): centro, meia-largura e maciez
float box(float y, float c, float hw, float soft)
{
    return smoothstep(hw + soft, hw, abs(y - c));
}

float3 studio(float3 r)
{
    // Cromo precisa de contraste duro: horizonte escuro/claro e softboxes com borda definida
    float y = r.y * 1.9 + r.x * 0.35;
    float horizon = smoothstep(-0.03, 0.03, y - 0.05);
    float3 col = lerp(float3(0.040, 0.050, 0.085), float3(0.11, 0.13, 0.19), smoothstep(-1.2, 0.0, y));   // o preto não é absoluto: tem leve fundo azulado
    // céu do estúdio: degradê longo (não satura), com variação lateral para não virar um platô chapado
    float sky = smoothstep(0.05, 3.2, y) + 0.12 * sin(r.x * 3.0 + y * 1.5);
    col += lerp(float3(0.03, 0.035, 0.05), float3(0.52, 0.58, 0.70), saturate(sky)) * horizon;
    col += float3(1.0, 0.98, 0.94) * box(y, 0.55, 0.12, 0.03) * 2.2;                 // softbox principal
    col += float3(1.0, 0.98, 0.94) * box(y, 1.05, 0.05, 0.02) * 1.6;                // tira fina
    col += float3(0.35, 0.55, 1.0) * box(y, -0.55, 0.09, 0.03) * 1.1;       // luz azul de preenchimento
    col += float3(1.0, 0.5, 0.25) * box(y, -0.95, 0.04, 0.02) * 0.9;                // fio quente no fundo
    // molduras escuras refletidas (janelas do estúdio): recortes finos dentro do céu claro
    col *= 1.0 - 0.85 * box(y, 0.95, 0.018, 0.012) - 0.6 * box(y, 1.45, 0.03, 0.015);
    return col;
}

float4 ps_main(float4 pos : SV_Position) : SV_Target
{
    float2 uv = frag_uv(pos);
    float2 p = cursor_lens(frag_p(pos), 0.22, 0.35) * 2.2 + parallax(0.2);

    float e = 0.01;
    float h = height_at(p);
    float hx = height_at(p + float2(e, 0.0));
    float hy = height_at(p + float2(0.0, e));
    float3 n = normalize(float3(-(hx - h) / e, -(hy - h) / e, 1.3));
    float3 v = float3(0.0, 0.0, 1.0);
    float3 r = reflect(-v, n);

    float3 col = studio(normalize(float3(r.x, r.y, max(r.z, 0.05))));
    float fres = 0.75 + 0.25 * pow(1.0 - saturate(dot(n, v)), 3.0);
    col *= fres * float3(0.95, 0.96, 1.0);

    col = aces(col * 1.2 * P(1));
    col *= 0.7 + 0.3 * vignette(uv, 0.35);
    return float4(finish(col, pos), 1.0);
}
