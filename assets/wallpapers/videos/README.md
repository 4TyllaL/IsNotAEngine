# Vídeos de fábrica

Wallpapers em vídeo que acompanham o app. Os arquivos de vídeo **não** vão para o git (são grandes);
só este README e os `.json` de metadados. O instalador e o pacote MSIX copiam o que estiver nesta pasta
para `videos\` ao lado do executável.

## Como adicionar

1. Coloque o vídeo aqui com um nome simples: letras, números, `_` ou `-` (ex.: `tokyo_alley.mp4`).
   Esse nome vira o id `video:tokyo_alley`, que a configuração guarda.
2. Opcional: um `tokyo_alley.json` ao lado:

   ```json
   { "title": "Beco em Tóquio", "category": "Ambient", "description": "Chuva, neon e reflexos no asfalto." }
   ```

   Sem o `.json`, o título vem do nome do arquivo e a categoria é "Ambient".
3. Rode o app de dentro da pasta do projeto (`cargo run`) para ver na galeria, ou gere o instalador/MSIX.

## Recomendações

- **Loop perfeito:** o último quadro deve encaixar no primeiro (o app volta ao início sem transição).
- **Formato:** H.264 (`.mp4`) 1920×1080, 24–30 fps, 8–20 s, sem áudio. O Windows decodifica por hardware.
- **Tamanho:** mire em 5–15 MB por vídeo (taxa de ~4–8 Mbit/s). Tudo isso entra no download da Store.
- **Direitos:** só vídeos que você gerou ou tem licença para redistribuir dentro do app.
