// Header comum prefixado automaticamente em todo shader do catálogo (e em shaders
// importados que não declaram o próprio cbuffer). O layout espelha `ShaderConstants`.
cbuffer Constants : register(b0)
{
    float2 resolution;
    float  time;
    float  speed;
    float4 tint;              // rgb + flag (w > 0 = cor de destaque escolhida no Studio)
    float2 mouse;             // 0..1, y para cima
    float2 mouse_click;
    float  mouse_enabled;
    float  parallax_enabled;
    float2 tint_ref;          // x = ângulo YIQ do matiz de referência do shader, y = 1 liga a rotação global (`// retint:`)
    float4 clock_time;        // hora, minuto, segundo, segundo contínuo
    float4 clock_date;        // ano, mês, dia, dia da semana
    float4 audio_reactive;    // graves, médios, agudos, volume
    float4 audio_spectrum[4]; // 16 bandas
    float4 params[2];         // parâmetros `// param:` na ordem de declaração
    float4 fade_ctl;          // x = escurecimento da transição da playlist (0 = normal, 1 = preto)
};

static const float PI = 3.14159265;
static const float TAU = 6.28318531;

float4 vs_main(uint id : SV_VertexID) : SV_Position
{
    float2 uv = float2((id << 1) & 2, id & 2);
    return float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
}

// Tempo escalado pela velocidade do Studio.
float T() { return time * (speed > 0.0 ? speed : 1.0); }

// Coordenadas: uv em 0..1 (y para cima) e p centrado com aspecto corrigido (altura = 1).
float2 frag_uv(float4 pos) { return float2(pos.x / resolution.x, 1.0 - pos.y / resolution.y); }
float2 frag_p(float4 pos)  { return (float2(pos.x, resolution.y - pos.y) - 0.5 * resolution) / resolution.y; }

// Deslocamento suave de parallax guiado pelo mouse (0 quando desativado).
float2 parallax(float depth)
{
    return parallax_enabled > 0.5 ? (mouse - 0.5) * depth : float2(0.0, 0.0);
}

// i-ésimo parâmetro de usuário (0..7).
float P(int i) { float4 v = params[clamp(i, 0, 7) >> 2]; int c = i & 3; return c == 0 ? v.x : c == 1 ? v.y : c == 2 ? v.z : v.w; }

float bass()   { return audio_reactive.x; }
float mids()   { return audio_reactive.y; }
float highs()  { return audio_reactive.z; }
float volume() { return audio_reactive.w; }

float band(int i)
{
    i = clamp(i, 0, 15);
    float4 v = audio_spectrum[i >> 2];
    int c = i & 3;
    return c == 0 ? v.x : c == 1 ? v.y : c == 2 ? v.z : v.w;
}

// Espectro amostrado de forma contínua (x em 0..1).
float spectrum(float x)
{
    float f = saturate(x) * 15.0;
    int i = (int)floor(f);
    return lerp(band(i), band(i + 1), smoothstep(0.0, 1.0, frac(f)));
}

// Cor de destaque: a escolhida no Studio ou o padrão do wallpaper.
// Com `// retint:` a rotação de matiz global já aplica a cor escolhida em `finish`; trocar também aqui aplicaria duas vezes
// (roxo viraria vermelho). Então, nesses shaders, accent() devolve a cor original e a rotação faz o resto.
float3 accent(float3 fallback) { return (tint.w > 0.0 && tint_ref.y < 0.5) ? tint.rgb : fallback; }

float hash11(float p)
{
    p = frac(p * 0.1031);
    p *= p + 33.33;
    p *= p + p;
    return frac(p);
}

float hash12(float2 p)
{
    float3 p3 = frac(float3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return frac((p3.x + p3.y) * p3.z);
}

float2 hash22(float2 p)
{
    float3 p3 = frac(float3(p.xyx) * float3(0.1031, 0.1030, 0.0973));
    p3 += dot(p3, p3.yzx + 33.33);
    return frac((p3.xx + p3.yz) * p3.zy);
}

float3 hash32(float2 p)
{
    float3 p3 = frac(float3(p.xyx) * float3(0.1031, 0.1030, 0.0973));
    p3 += dot(p3, p3.yxz + 33.33);
    return frac((p3.xxy + p3.yzz) * p3.zyx);
}

float noise(float2 p)
{
    float2 i = floor(p);
    float2 f = frac(p);
    float2 u = f * f * (3.0 - 2.0 * f);
    return lerp(lerp(hash12(i), hash12(i + float2(1, 0)), u.x),
                lerp(hash12(i + float2(0, 1)), hash12(i + float2(1, 1)), u.x), u.y);
}

float noise3(float3 p)
{
    float3 i = floor(p);
    float3 f = frac(p);
    float3 u = f * f * (3.0 - 2.0 * f);
    float n = i.x + i.y * 157.0 + i.z * 113.0;
    float a = hash11(n), b = hash11(n + 1.0), c = hash11(n + 157.0), d = hash11(n + 158.0);
    float e = hash11(n + 113.0), f1 = hash11(n + 114.0), g = hash11(n + 270.0), h = hash11(n + 271.0);
    return lerp(lerp(lerp(a, b, u.x), lerp(c, d, u.x), u.y),
                lerp(lerp(e, f1, u.x), lerp(g, h, u.x), u.y), u.z);
}

float2x2 rot(float a)
{
    float c = cos(a), s = sin(a);
    return float2x2(c, -s, s, c);
}

float fbm(float2 p)
{
    float v = 0.0, a = 0.5;
    float2x2 m = float2x2(1.6, 1.2, -1.2, 1.6);
    for (int i = 0; i < 6; i++)
    {
        v += a * noise(p);
        p = mul(m, p);
        a *= 0.5;
    }
    return v;
}

float fbm3(float3 p)
{
    float v = 0.0, a = 0.5;
    for (int i = 0; i < 5; i++)
    {
        v += a * noise3(p);
        p = p * 2.02 + float3(1.7, 9.2, 4.1);
        a *= 0.5;
    }
    return v;
}

// Paleta cosseno de Inigo Quilez.
float3 palette(float t, float3 a, float3 b, float3 c, float3 d)
{
    return a + b * cos(TAU * (c * t + d));
}

// Pequeno dithering para evitar banding em gradientes escuros.
float3 dither(float3 col, float4 pos)
{
    return col + (hash12(pos.xy + frac(time) * 61.0) - 0.5) / 255.0;
}

float3 aces(float3 x)
{
    return saturate((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14));
}

float vignette(float2 uv, float k)
{
    float2 d = uv * (1.0 - uv);
    return pow(saturate(d.x * d.y * 16.0), k);
}

// Ombro suave: comprime só os realces (acima de k) em vez de clipar duro.
float3 soft_clip(float3 c, float k)
{
    float3 over = max(c - k, 0.0);
    return min(c, k) + over / (1.0 + over / (1.0 - k));
}

// Acabamento "premium" aplicado no final de todo shader do catálogo:
// realces sem clipping, leve curva S de contraste, saturação sutil,
// grão de filme concentrado nos tons médios e dithering triangular (TPDF).
// Gira o matiz da imagem (espaço YIQ, luminância intacta) do matiz de referência do shader para o do tint.
float3 retint_rotate(float3 col)
{
    float3 t = tint.rgb;
    float ta = atan2(dot(t, float3(0.211, -0.523, 0.312)), dot(t, float3(0.596, -0.274, -0.322)));
    float d = ta - tint_ref.x;
    float y = dot(col, float3(0.299, 0.587, 0.114));
    float i = dot(col, float3(0.596, -0.274, -0.322));
    float q = dot(col, float3(0.211, -0.523, 0.312));
    float c = cos(d), s = sin(d);
    float i2 = i * c - q * s, q2 = i * s + q * c;
    return max(float3(y + 0.956 * i2 + 0.621 * q2, y - 0.272 * i2 - 0.647 * q2, y - 1.106 * i2 + 1.703 * q2), 0.0);
}

float3 finish(float3 col, float4 pos)
{
    if (tint.w > 0.0 && tint_ref.y > 0.5) col = retint_rotate(col);
    col = soft_clip(max(col, 0.0), 0.82);

    float l = dot(col, float3(0.2126, 0.7152, 0.0722));
    float3 s = col * col * (3.0 - 2.0 * col);           // curva S
    col = lerp(col, s, 0.12);
    col = max(lerp(l.xxx, col, 1.06), 0.0);              // +6% saturação

    float2 seed = pos.xy + frac(time * 0.37) * 517.0;
    float grain = hash12(seed) + hash12(seed + 71.3) - 1.0;
    col += grain * 0.022 * (l * (1.0 - l) * 3.0 + 0.08);  // grão maior nos médios

    float tpdf = hash12(seed * 1.31 + 3.7) + hash12(seed * 0.73 + 9.1) - 1.0;
    return saturate(col + tpdf / 255.0) * (1.0 - saturate(fade_ctl.x));
}

// ---------- Interação com o mouse (desligada no Studio = cursor sem efeito) ----------
// Cursor no mesmo espaço de `frag_p` (centro = 0, altura = 1).
float2 mouse_p() { return (mouse - 0.5) * float2(resolution.x / resolution.y, 1.0); }

// Botão pressionado (esquerdo ou direito), 0 com a interação desligada.
float mouse_down() { return mouse_enabled > 0.5 ? max(mouse_click.x, mouse_click.y) : 0.0; }

// Influência do cursor em `p`: 1 sobre ele, caindo suavemente até ~0 a `radius`.
float cursor_field(float2 p, float radius)
{
    if (mouse_enabled < 0.5) return 0.0;
    float2 d = p - mouse_p();
    return exp(-dot(d, d) / (radius * radius));
}

// Redemoinho em volta do cursor (`amount` em radianos no centro); segurar o clique intensifica.
// Use em `frag_p(pos)` antes de escalar/aplicar parallax.
float2 cursor_swirl(float2 p, float radius, float amount)
{
    float k = cursor_field(p, radius);
    if (k < 1e-3) return p;
    float2 c = mouse_p();
    return c + mul(rot(amount * k * (1.0 + 1.5 * mouse_down())), p - c);
}

// Lente que afasta (amount > 0) ou puxa o conteúdo em volta do cursor.
float2 cursor_lens(float2 p, float radius, float amount)
{
    float k = cursor_field(p, radius) * (1.0 + mouse_down());
    return lerp(p, mouse_p(), -amount * k);
}
