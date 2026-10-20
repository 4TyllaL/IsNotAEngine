// Chuva na janela: gotas que escorrem em zigue-zague deixando rastro, gotículas paradas no vidro e refração real:
// dentro da gota a cidade aparece nítida e invertida em cor; fora, desfocada pelo vidro embaçado.
// param: rain 0 1 0.75 -- Quantidade de chuva
// param: fog 0 1 0.5 -- Névoa no vidro
// param: lights 0.4 1.8 1.25 -- Brilho das luzes da cidade
// Dente de serra suave: sobe rápido, desce devagar (ciclo de vida da gota)
float saw(float b, float t) { return smoothstep(0.0, b, t) * smoothstep(1.0, b, t); }

float3 n13(float p)
{
    float3 p3 = frac(float3(p, p, p) * float3(0.1031, 0.11369, 0.13787));
    p3 += dot(p3, p3.yzx + 19.19);
    return frac(float3((p3.x + p3.y) * p3.z, (p3.x + p3.z) * p3.y, (p3.y + p3.z) * p3.x));
}

// Uma camada de gotas grandes escorrendo. x = máscara da gota, y = rastro molhado.
float2 drop_layer(float2 uv, float t)
{
    float2 UV = uv;
    uv.y += t * 0.75;
    float2 a = float2(6.0, 1.0);
    float2 grid = a * 2.0;
    float2 id = floor(uv * grid);
    uv.y += hash11(id.x * 7.31);
    id = floor(uv * grid);
    float3 n = n13(id.x * 35.2 + id.y * 2376.1);
    float2 st = frac(uv * grid) - float2(0.5, 0.0);

    float x = n.x - 0.5;
    float y = UV.y * 20.0;
    float wiggle = sin(y + sin(y));
    x += wiggle * (0.5 - abs(x)) * (n.z - 0.5);
    x *= 0.7;
    float ti = frac(t + n.z);
    y = (saw(0.85, ti) - 0.5) * 0.9 + 0.5;
    float2 p = float2(x, y);

    float d = length((st - p) * a.yx);
    float main_drop = smoothstep(0.4, 0.0, d);

    float r = sqrt(smoothstep(1.0, y, st.y));
    float cd = abs(st.x - x);
    float trail = smoothstep(0.23 * r, 0.15 * r * r, cd);
    float trail_front = smoothstep(-0.02, 0.02, st.y - y);
    trail *= trail_front * r * r;

    // Gotículas deixadas pelo rastro
    float yy = frac(UV.y * 10.0) + (st.y - 0.5);
    float droplets = smoothstep(0.3, 0.0, length(st - float2(x, yy)));
    return float2(main_drop + droplets * r * trail_front, trail);
}

float static_drops(float2 uv, float t)
{
    uv *= 40.0;
    float2 id = floor(uv);
    uv = frac(uv) - 0.5;
    float3 n = n13(id.x * 107.45 + id.y * 3543.654);
    float d = length(uv - (n.xy - 0.5) * 0.7);
    float fade = saw(0.025, frac(t + n.z));
    return smoothstep(0.3, 0.0, d) * frac(n.z * 10.0) * fade;
}

float2 drops(float2 uv, float t, float l0, float l1, float l2)
{
    float s = static_drops(uv, t) * l0;
    float2 m1 = drop_layer(uv, t) * l1;
    float2 m2 = drop_layer(uv * 1.85, t) * l2;
    float c = smoothstep(0.3, 1.0, s + m1.x + m2.x);
    return float2(c, max(m1.y * l0, m2.y * l1));
}

// Altura do prédio `id` (com recuo no topo de alguns) na posição local lx.
float building_top(float id, float lx, float fk)
{
    float hb = hash11(id * 3.1 + fk * 17.0);
    float bh = 0.22 + 0.42 * hb * hb + (0.12 - fk * 0.08);
    return bh + step(0.6, hash11(id * 9.7 + fk)) * step(abs(lx - 0.5), 0.22) * 0.04;
}

// Cidade noturna com desfoque controlável (0 = nítida, 1 = muito desfocada).
float3 city(float2 uv, float blur)
{
    float3 col = lerp(float3(0.16, 0.08, 0.10), float3(0.05, 0.045, 0.10), saturate(uv.y * 1.3));
    col += float3(0.35, 0.16, 0.08) * exp(-abs(uv.y - 0.3) * 7.0) * 0.35; // poluição luminosa

    // Três camadas de prédios (fundo -> frente), larguras variadas e névoa por profundidade
    for (int k = 0; k < 3; k++)
    {
        float fk = (float)k;
        float x = uv.x * (9.0 - fk * 2.5) + fk * 13.7;
        float cell = floor(x);
        float off = (hash11(cell * 1.7 + fk) - 0.5) * 0.6;   // desloca a divisa entre prédios
        float id = floor(x + off);
        float lx = frac(x + off);
        float top = building_top(id, lx, fk);
        // Desfoque horizontal: a altura se mistura com a dos vizinhos na divisa
        float w = 0.01 + blur * 0.45;
        top = lerp(building_top(id - 1.0, 1.0, fk), top, smoothstep(0.0, w, lx) * 0.5 + 0.5);
        top = lerp(building_top(id + 1.0, 0.0, fk), top, smoothstep(0.0, w, 1.0 - lx) * 0.5 + 0.5);
        float edge = 0.0015 + blur * 0.05;
        float building = smoothstep(top + edge, top - edge, uv.y);

        float haze = 1.0 - fk / 3.0;
        float3 wall = lerp(float3(0.008, 0.008, 0.016), float3(0.05, 0.035, 0.06), haze * haze * 0.8);
        col = lerp(col, wall, building);

        // Janelas: nítidas quando em foco, viram brilho médio quando desfocadas
        float2 wg = float2(lx * (6.0 + fk * 2.0), uv.y * (70.0 - fk * 15.0));
        float2 wid = floor(wg);
        float2 wf = frac(wg) - 0.5;
        float hw = hash12(wid + id * 11.0 + fk * 5.0);
        float lit = step(0.72, hw);
        float aa = fwidth(wg.y) + blur * 0.6;
        float shape = smoothstep(0.32 + aa, 0.32 - aa, max(abs(wf.x) * 1.2, abs(wf.y)));
        float sharp = shape * lit;
        float soft = 0.28 * 0.45;                              // média de janelas acesas
        float win = lerp(sharp, soft, saturate(blur * 1.4));
        float inside = building * smoothstep(top - 0.01, top - 0.02 - blur * 0.05, uv.y) * lerp(step(0.12, lx) * step(lx, 0.88), 0.76, saturate(blur * 1.4));
        float3 wc = lerp(float3(1.0, 0.72, 0.38), float3(0.75, 0.85, 1.0), step(0.93, hw));
        col += wc * win * inside * (0.22 - fk * 0.05) * haze;
    }

    // Bokeh: luzes de rua e faróis
    for (int i = 0; i < 3; i++)
    {
        float fi = (float)i;
        float2 g = uv * float2(7.0 + fi * 4.0, 4.0 + fi * 2.0) + fi * 7.3;
        float2 id = floor(g);
        float2 f = frac(g) - 0.5;
        float h = hash12(id);
        float d = length(f - (hash22(id) - 0.5) * 0.5);
        float rad = (0.04 + blur * 0.3) * (0.55 + 0.7 * frac(h * 23.7));
        float soft = 0.01 + blur * 0.12;
        float disc = smoothstep(rad, rad - soft, d) * (0.45 + 0.35 * smoothstep(rad * 0.5, rad, d) * blur);
        float3 c = lerp(float3(1.0, 0.62, 0.28), accent(float3(0.4, 0.62, 1.0)), step(0.6, h));
        c = lerp(c, float3(1.0, 0.28, 0.3), step(0.88, h));
        float on = step(0.5, h) * smoothstep(0.75, 0.15, uv.y);
        float tw = 0.85 + 0.15 * sin(T() * (0.5 + h) + h * 30.0);
        col += c * disc * on * tw * (0.9 - fi * 0.22) / (1.0 + blur * 3.0) * 1.6 * P(2);
    }
    return col;
}

float4 ps_main(float4 pos : SV_Position) : SV_Target
{
    float2 UV = frag_uv(pos);
    float2 uv = frag_p(pos) * 0.7;
    float t = T() * 0.2;

    float rain = P(0);
    float l0 = smoothstep(-0.5, 1.0, rain) * 2.0;
    float l1 = smoothstep(0.25, 0.75, rain);
    float l2 = smoothstep(0.0, 0.5, rain);

    float2 c = drops(uv, t, l0, l1, l2);
    // Normal da superfície da água via diferenças finitas da máscara
    float2 e = float2(0.0009, 0.0);
    float cx = drops(uv + e, t, l0, l1, l2).x;
    float cy = drops(uv + e.yx, t, l0, l1, l2).x;
    float2 n = float2(cx - c.x, cy - c.x);

    // Vidro embaçado fora das gotas; o rastro molhado limpa parcialmente
    float focus = lerp(1.0 - c.y * 0.35, 0.0, smoothstep(0.1, 0.2, c.x)) * lerp(0.55, 1.0, P(1));
    float2 buv = UV + parallax(0.02);
    // aberração cromática: dentro das gotas cada canal vê a cidade com um desvio levemente diferente
    float inD = smoothstep(0.1, 0.2, c.x);
    float3 col = city(buv + n * 1.2, focus);
    float3 ca = float3(city(buv + n * 1.12, 0.0).r, col.g, city(buv + n * 1.28, 0.0).b);   // sem ramificação: city usa derivadas
    col = lerp(col, ca, inD);

    // Brilho especular na borda das gotas
    float3 nn = normalize(float3(n * 30.0, 1.0));
    col += float3(0.9, 0.9, 1.0) * pow(saturate(dot(nn, normalize(float3(-0.4, 0.6, 0.7)))), 30.0) * c.x * 0.35;
    // Leve embaçado frio no vidro; dentro das gotas a luz é concentrada (lente)
    float inDrop = smoothstep(0.1, 0.2, c.x);
    col = lerp(col, float3(0.10, 0.09, 0.13), (1.0 - inDrop) * (1.0 - c.y) * 0.12);
    col *= 1.0 + inDrop * 1.0;
    // contorno escuro fino na borda de cada gota, como nas fotos
    col *= 1.0 - 0.38 * saturate(length(n) * 28.0) * (1.0 - smoothstep(0.55, 0.9, c.x));

    col = aces(col * 1.7);
    col *= 0.7 + 0.3 * vignette(UV, 0.3);
    return float4(finish(col, pos), 1.0);
}
