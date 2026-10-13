"""宿主 Chat Completions 配置与 browser-use 结构化输出之间的适配。"""

import json

from browser_use import ChatOpenAI


JSON_OUTPUT_CONTRACT = """\
Response protocol: return exactly one valid JSON object matching the JSON schema below.
Do not return prose, XML tags, Markdown code fences, or tool-call syntax.
"""

BROWSER_OUTPUT_CONTRACT = """\
All browser actions must be inside the action array. Put the user's requested final
answer and formatting inside the done action's text field, never outside the JSON object.
"""


class CompatibleChatOpenAI(ChatOpenAI):
    def get_client(self):
        client = super().get_client()
        create = client.chat.completions.create

        async def create_compatible(*args, **kwargs):
            # BU 只暴露 max_completion_tokens；宿主使用 max_tokens 语义。
            cap = kwargs.pop("max_completion_tokens", None)
            if cap is not None:
                kwargs["max_tokens"] = cap
            output = kwargs.get("response_format")
            if output and output.get("type") == "json_schema":
                # 通用网关不一定支持 strict json_schema；JSON mode 约束语法，
                # SDK 提供的动作 schema + 上游 Pydantic 校验约束实际结构。
                schema = output["json_schema"]["schema"]
                protocol = JSON_OUTPUT_CONTRACT
                if "action" in schema.get("properties", {}):
                    protocol += BROWSER_OUTPUT_CONTRACT
                instruction = (
                    f"{protocol}\n<json_schema>\n"
                    f"{json.dumps(schema, ensure_ascii=False)}\n</json_schema>"
                )
                messages = list(kwargs["messages"])
                if messages and messages[0]["role"] == "system":
                    first = dict(messages[0])
                    content = first["content"]
                    first["content"] = (
                        content + "\n\n" + instruction
                        if isinstance(content, str)
                        else [*content, {"type": "text", "text": instruction}]
                    )
                    messages[0] = first
                else:
                    messages.insert(0, {"role": "system", "content": instruction})
                kwargs["messages"] = messages
                kwargs["response_format"] = {"type": "json_object"}
            return await create(*args, **kwargs)

        client.chat.completions.create = create_compatible
        return client


def model_from_config(config):
    if config.get("apiType", "openai") != "openai":
        raise ValueError("浏览器探索仅支持 OpenAI-compatible Chat Completions")
    model = str(config.get("model", "")).strip()
    base = str(config.get("apiBase", "")).strip().rstrip("/")
    if not model or not base:
        raise ValueError("浏览器探索模型及 API 地址不能为空")
    if base.endswith("/chat/completions"):
        base = base[: -len("/chat/completions")]
    return CompatibleChatOpenAI(
        model=model,
        api_key=config.get("apiKey") or "not-configured",
        base_url=base,
        max_completion_tokens=config.get("maxTokens"),
        temperature=config.get("temperature"),
        frequency_penalty=None,
        reasoning_models=[],
        add_schema_to_system_prompt=False,
        dont_force_structured_output=False,
        max_retries=1,
    )
