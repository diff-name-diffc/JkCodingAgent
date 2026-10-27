use serde_json::{json, Value};

fn source_schema() -> Value {
    json!({
        "oneOf": [
            {
                "type":"object", "additionalProperties":false,
                "properties":{
                    "type":{"const":"docker"},
                    "server_id":{"type":"string","minLength":1},
                    "container":{"type":"string","minLength":1,"maxLength":128},
                    "sudo":{"type":"boolean","default":false}
                },
                "required":["type","server_id","container"]
            },
            {
                "type":"object", "additionalProperties":false,
                "properties":{
                    "type":{"const":"ssh_file"},
                    "server_id":{"type":"string","minLength":1},
                    "path":{"type":"string","minLength":1},
                    "sudo":{"type":"boolean","default":false}
                },
                "required":["type","server_id","path"]
            },
            {
                "type":"object", "additionalProperties":false,
                "properties":{
                    "type":{"const":"local_file"},
                    "path":{"type":"string","minLength":1}
                },
                "required":["type","path"]
            }
        ]
    })
}

pub(super) fn update_parameters() -> Value {
    json!({
        "type":"object", "additionalProperties":false,
        "properties":{
            "snapshot_id":{"type":"string","description":"已存在的快照 ID；省略时创建新快照。更新成功后 ID 不变、generation 增加。"},
            "label":{"type":"string","maxLength":128,"description":"可选的排障主题名称。"},
            "sources":{
                "type":"array","maxItems":8,
                "description":"Docker、SSH 文件、本地文件来源。首次创建必填；后续省略则复用上一代全部来源。",
                "items":source_schema()
            },
            "since":{"type":"string","description":"仅 Docker：RFC3339 或 30m/2h/1d。每次 update 重新计算相对时间。"},
            "until":{"type":"string","description":"仅 Docker：RFC3339 或相对时间；默认冻结为本次更新开始时间。"},
            "tail":{"type":"integer","minimum":1,"maximum":1000000,"description":"仅 Docker：可选末尾行数；需要完整时间窗时不要设置。"},
            "max_total_bytes":{"type":"integer","minimum":1048576,"maximum":68719476736i64,"description":"所有来源原始日志总字节上限，默认 2 GiB。索引另有独立上限。"},
            "timeout_secs":{"type":"integer","minimum":1,"maximum":21600,"description":"后台更新总预算，默认 1800 秒；不是单次工具等待时间。"}
        }
    })
}

pub(super) fn discover_parameters() -> Value {
    json!({
        "type":"object", "additionalProperties":false,
        "properties":{
            "server_id":{"type":"string","description":"需要发现 Docker 或远端文件时必填，来自 ssh_list_servers。"},
            "include_docker":{"type":"boolean","default":true,"description":"server_id 存在时是否列出容器，默认 true。"},
            "remote_files":{"type":"array","maxItems":8,"items":{"type":"string"},"description":"要确认存在性和大小的远端绝对文件路径。"},
            "local_files":{"type":"array","maxItems":8,"items":{"type":"string"},"description":"会话目录或授权工程目录内的本地文件。"},
            "sudo":{"type":"boolean","default":false,"description":"远端发现命令是否提权；仅在确有需要时使用。"},
            "timeout_secs":{"type":"integer","minimum":1,"maximum":300,"description":"单条远端发现命令超时，使用服务器默认时可省略。"}
        }
    })
}

pub(super) fn status_parameters() -> Value {
    json!({
        "type":"object", "additionalProperties":false,
        "properties":{
            "snapshot_id":{"type":"string","description":"省略时列出当前会话全部快照。"},
            "wait_secs":{"type":"integer","minimum":0,"maximum":30,"description":"有活动更新时等待阶段变化，默认 0，最大 30 秒。"}
        }
    })
}

pub(super) fn cancel_parameters() -> Value {
    json!({
        "type":"object", "additionalProperties":false,
        "properties":{"snapshot_id":{"type":"string"}},
        "required":["snapshot_id"]
    })
}

pub(super) fn search_parameters() -> Value {
    json!({
        "type":"object", "additionalProperties":false,
        "properties":{
            "snapshot_id":{"type":"string"},
            "pattern":{"type":"string","description":"单个搜索模式；与 patterns 二选一。"},
            "patterns":{"type":"array","minItems":1,"maxItems":4,"items":{"type":"string"},"description":"多个模式按 OR 匹配。"},
            "match_mode":{"type":"string","enum":["fixed","regex"],"default":"fixed","description":"fixed 为字面量子串；regex 走本地精确扫描。"},
            "case_sensitive":{"type":"boolean","default":false},
            "output_mode":{"type":"string","enum":["files","count","matches"],"default":"matches","description":"先 files/count 收敛范围，再 matches 获取定位证据。"},
            "engine":{"type":"string","enum":["auto","scan"],"default":"auto","description":"auto 自动选择全文索引或精确扫描；scan 强制穷举扫描。"},
            "file_ids":{"type":"array","maxItems":8,"items":{"type":"integer","minimum":1},"description":"只查指定来源文件。"},
            "context_before":{"type":"integer","minimum":0,"maximum":20,"default":0},
            "context_after":{"type":"integer","minimum":0,"maximum":20,"default":0},
            "max_matches":{"type":"integer","minimum":1,"maximum":200,"default":50},
            "max_files":{"type":"integer","minimum":1,"maximum":200,"default":50},
            "max_line_chars":{"type":"integer","minimum":32,"maximum":20000,"default":2000},
            "cursor":{"type":"string","description":"上一页 next_cursor；绑定 generation 和查询条件。"}
        },
        "required":["snapshot_id"],
        "anyOf":[{"required":["pattern"]},{"required":["patterns"]}]
    })
}

pub(super) fn read_parameters() -> Value {
    json!({
        "type":"object", "additionalProperties":false,
        "properties":{
            "snapshot_id":{"type":"string"},
            "file_id":{"type":"integer","minimum":1},
            "line_number":{"type":"integer","minimum":1,"description":"目标行；与 byte_offset 二选一。均省略时从第 1 行开始。"},
            "before":{"type":"integer","minimum":0,"maximum":100,"default":0,"description":"目标行之前的上下文行数；注意 before+after+1 合计不得超过 500。"},
            "after":{"type":"integer","minimum":0,"maximum":499,"default":99,"description":"目标行之后的读取行数；分页时用 next_line 并把 before 设为 0；注意 before+after+1 合计不得超过 500。"},
            "byte_offset":{"type":"integer","minimum":0,"description":"原始字节偏移，适合超长行；不能与 line_number 同用。"},
            "max_bytes":{"type":"integer","minimum":1024,"maximum":1048576,"default":65536},
            "max_line_chars":{"type":"integer","minimum":32,"maximum":20000,"default":2000}
        },
        "required":["snapshot_id","file_id"]
    })
}
