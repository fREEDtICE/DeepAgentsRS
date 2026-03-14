# Cross‑Platform LLM Prompt Cache Design Guide (2026)

This document summarizes how major LLM platforms implement **Prompt Cache / Prefix Cache / Context Cache / KV Cache**, and evaluates a common engineering strategy:

> Splitting **static prompts** and **dynamic prompts** into multiple system messages to increase Prompt Cache hit rates.

Covered platforms:

- OpenAI
- Azure OpenAI
- Anthropic Claude
- Google Gemini
- OpenRouter
- Alibaba Qwen
- Volcano Engine Doubao
- MiniMax
- Moonshot (Kimi)
- Zhipu GLM
- AWS Bedrock
- Microsoft Copilot
- Ollama

---

# 1. Core Principle

Across nearly all providers, Prompt Cache works on the same fundamental rule:

**If the prefix tokens of a request are identical, the model may reuse previous computation.**

Therefore the key design principles are:

1. **Stable prefix must appear first**
2. **Dynamic content should be placed later**
3. **Tool schemas and system policies should remain stable**
4. **Do not change message ordering**

A typical prompt structure:

```
SYSTEM_STATIC
SYSTEM_DYNAMIC
TOOLS_SCHEMA
USER
```

---

# 2. Prompt Cache Technology Types

Different providers implement caching using several technical approaches.

## 2.1 Prefix Cache

The server stores a hash of prompt prefixes.

```
request
 ├── prefix tokens
 └── new tokens
```

If the prefix matches a previous request:

```
prefix compute reused
```

Representative providers:

- OpenAI
- MiniMax
- GLM

---

## 2.2 Context Cache

The platform allows developers to explicitly cache a block of context.

```
create_cache(prompt)
 → cache_id

request(cache_id + new_prompt)
```

Representative providers:

- Gemini
- Moonshot
- Doubao

---

## 2.3 Cache Breakpoint

Developers explicitly define where caching should occur in the prompt.

Representative provider:

- Anthropic Claude

---

## 2.4 KV Cache (Inference-Level Cache)

The model internally reuses attention KV states.

```
prefix tokens
 → KV states
```

Representative platforms:

- Ollama
- llama.cpp

---

# 3. Platform Support Matrix

| Platform | Multiple System Messages | Automatic Prefix Cache | Explicit Cache API | Recommended Strategy |
|---|---|---|---|---|
| OpenAI | ✓ | ✓ | ✗ | Stable prefix |
| Azure OpenAI | ✓ | Unstable | ✗ | Prefix optimization |
| Claude | ✗ | ✗ | ✓ | cache_control |
| Gemini | ✗ | Partial | ✓ | context caching |
| OpenRouter | Depends on upstream | Depends | Depends | provider-specific |
| MiniMax | ✓ | ✓ | ✗ | prefix |
| Moonshot | ✓ | Partial | ✓ | context cache |
| GLM | ✓ | ✓ | ✗ | prefix |
| Doubao | ✓ | Partial | ✓ | common_prefix |
| Alibaba Qwen | ✓ | Not clearly documented | Not clear | benchmark |
| Bedrock | Depends on model | ✗ | ✗ | minimal benefit |
| Copilot | ✗ | ✗ | ✗ | not controllable |
| Ollama | ✓ | KV reuse | ✗ | prefix reuse |

---

# 4. Strategy: Multiple System Messages

Strategy:

```
system: static prompt
system: dynamic prompt
user: question
```

## Advantages

- Stable prefix
- Dynamic section isolated
- Clear engineering structure

## Observed Effectiveness

| Platform | Effect |
|---|---|
| OpenAI | Effective |
| MiniMax | Effective |
| GLM | Effective |
| Ollama | Very effective |
| Gemini | Moderate |
| Moonshot | Moderate |
| Doubao | Moderate |
| Azure OpenAI | Unstable |
| Bedrock | Minimal benefit |
| Copilot | Not controllable |

---

# 5. Recommended Cross‑Platform Prompt Structure

A unified prompt template:

```
SYSTEM_STATIC
SYSTEM_RULES
SYSTEM_TOOLS
USER_CONTEXT
USER_QUERY
```

Explanation:

- `SYSTEM_STATIC`: long‑term stable instructions
- `SYSTEM_RULES`: business or product rules
- `SYSTEM_TOOLS`: tool schema definitions
- `USER_CONTEXT`: conversation history
- `USER_QUERY`: user input

---

# 6. Cross‑Platform Cache Strategy

Recommended approach:

## Step 1

Create a stable prompt prefix

```
base_prompt
```

---

## Step 2

Provider‑specific optimization

| Platform | Optimization |
|---|---|
| OpenAI | prefix reuse |
| Claude | cache_control |
| Gemini | cached_content |
| Moonshot | context cache |
| Doubao | common_prefix |
| MiniMax | automatic cache |
| Ollama | KV reuse |

---

# 7. Sources of Performance Gains

Caching benefits typically come from:

1. Reduced prefill latency
2. Reduced token computation
3. Lower inference cost

Typical improvements:

| Model | Latency Reduction |
|---|---|
| GPT‑4 class models | 30–60% |
| Claude models | 40–70% |
| Gemini models | 30–50% |
| Local models | 50%+ |

---

# 8. Common Misconceptions

## Misconception 1

"Multiple system messages enable caching"

In reality:

**Caching depends on identical token prefixes.**

---

## Misconception 2

"System messages are more cacheable than user messages"

In reality:

Cache behavior is independent of role type.

---

## Misconception 3

"All LLM platforms support prompt caching"

In reality many platforms:

- do not support caching
- or routing makes caching ineffective

---

# 9. Recommended Architecture for Multi‑Model Gateways

A practical architecture for multi‑model systems:

```
Prompt Builder
     ↓
Prompt Cache Layer
     ↓
Provider Adapter
     ↓
LLM Provider
```

Responsibilities of the cache layer:

- prefix hashing
- provider cache ID management
- prompt normalization

---

# 10. Summary

The key rule of Prompt Cache:

**Stable prefix > message count > role type**

Best practices:

1. Always place fixed prompts first
2. Keep dynamic prompts short
3. Use provider‑native cache APIs when available
4. Avoid frequently modifying tool schemas

In cross‑platform systems:

> Prompt Cache design often matters more than prompt engineering itself.

