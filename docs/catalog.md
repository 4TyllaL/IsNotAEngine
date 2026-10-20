# Catálogo embutido

Política: a biblioteca inicial é **do projeto**. Todo wallpaper embutido (`src/wallpaper/catalog.rs`)
foi escrito para o IsNotAEngine, sem código ou assets de terceiros. O que vier de fora entra pelas mãos
do usuário (Criar › Importar arquivo, Shadertoy, página da internet), nunca por distribuição nossa.
Distribuição remota de conteúdo não existe e exigiria um projeto de confiança próprio (conteúdo
ativo pode executar comportamento indesejado).

Licença do conteúdo embutido: **MIT**, a mesma do projeto (arquivo `LICENSE`).

## Ficha

Colunas: tipo; áudio (reage ao áudio do sistema); dados (lê dados da máquina); parâmetros
(ajustáveis em Ajustes › Ajustar); desempenho: **não medido** até B01 (ver `docs/performance.md`).
Movimento: baixo = quase parado, médio = fluxo contínuo, alto = muda rápido.

| id | tipo | áudio | dados | movimento | legibilidade de ícones |
|---|---|---|---|---|---|
| matrix | web | sim | não | alto | média (chuva densa) |
| coding | web | não | não | médio | baixa |
| system | web | não | sim (CPU, memória, disco, rede) | baixo | média |
| terminal_build | web | não | não | baixo | média |
| git_graph | web | não | não | baixo | média |
| clock_mono | web | não | não | baixo | alta |
| terminal_boot | web | não | não | médio | baixa |
| descriptografia | web | não | não | médio | média |
| aurora | shader | não | não | baixo | alta |
| night_sky | shader | não | não | baixo | alta |
| topo_dark | shader | não | não | baixo | alta |
| rain_glass | shader | não | não | médio | média |
| liquid_chrome | shader | não | não | médio | média |
| plasma_flow | shader | não | não | médio | alta |
| smoke | shader | não | não | baixo | alta |
| ocean | shader | não | não | baixo | alta |
| gyroid_silk | shader | não | não | médio | média |

Movimento e legibilidade são estimativas do autor, não medições. Permissões: o `system` lê dados da
máquina; nenhum wallpaper embutido precisa de rede (não verificado por teste). Não existe wallpaper por endereço da
internet. A tabela acima é conferida por teste contra
`CATALOG`: todo id do catálogo precisa de uma linha aqui.
