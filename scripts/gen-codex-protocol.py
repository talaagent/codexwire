#!/usr/bin/env python3
"""Generate codexwire's protocol types from Codex's app-server JSON Schema.

Input: the committed schema bundle (`codex app-server generate-json-schema`,
see `scripts/gen-codex-protocol`) and its VERSION file. Output: serde types
for the methods and notifications codexwire uses (ROOTS below and every
type they reach), plus `Request`/`Notification`/`ServerRequest` impls that
tie each method name to its params and response. Stdlib only, offline.

Shapes: objects become structs (optional or nullable fields `Option`),
string enums and `oneOf`s become enums with an `Unknown(Value)` catch-all
(a newer Codex adding a variant must not break decoding), `oneOf`s tagged
by a `type` field become internally tagged enums; anything else (mixed
`anyOf`, untyped) stays `serde_json::Value`.
"""

import json
import keyword
import re
import sys

# Client requests (method -> params; response is <Name>Response).
REQUESTS = [
    "initialize",
    "thread/start",
    "thread/resume",
    "turn/start",
    "turn/interrupt",
]
# Server notifications codexwire reads.
NOTIFICATIONS = [
    "error",
    "thread/started",
    "turn/started",
    "turn/completed",
    "item/started",
    "item/completed",
    "item/agentMessage/delta",
    "thread/tokenUsage/updated",
    "account/rateLimits/updated",
]
# Server requests codexwire answers.
SERVER_REQUESTS = [
    "item/commandExecution/requestApproval",
    "item/fileChange/requestApproval",
    "mcpServer/elicitation/request",
]
# Kept as raw JSON: big, and codexwire never looks inside.
OPAQUE = {
    "SandboxPolicy",
    "GitInfo",
    "ThreadStatus",
    "ActivePermissionProfile",
    "McpServerElicitationRequestParams",
    "CollabAgentState",
    "WebSearchAction",
    "HookPromptFragment",
    "MemoryCitation",
    "AsyncUserInputQuestion",
    "FunctionCallOutputBody",
    "McpToolCallResult",
    "DynamicToolCallOutputContentItem",
    "CommandAction",
    "NetworkPolicyAmendment",
    "AdditionalPermissionProfile",
    "TextElement",
}

RUST_KW = set(keyword.kwlist) | {
    "type",
    "ref",
    "match",
    "move",
    "mod",
    "self",
    "super",
    "crate",
    "loop",
    "use",
    "where",
    "impl",
    "trait",
    "fn",
    "let",
    "mut",
    "pub",
    "static",
    "struct",
    "enum",
    "extern",
    "unsafe",
    "dyn",
    "abstract",
    "final",
    "override",
    "macro",
    "box",
    "priv",
    "typeof",
    "unsized",
    "virtual",
    "yield",
    "try",
    "async",
    "await",
    "const",
}


def snake(name):
    s = re.sub(r"[^0-9A-Za-z]+", "_", name).strip("_")
    s = re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", s)
    s = re.sub(r"([A-Z]+)([A-Z][a-z])", r"\1_\2", s).lower()
    if not s or s[0].isdigit():
        s = "f_" + s
    return f"r#{s}" if s in RUST_KW else s


def pascal(name):
    parts = re.split(r"[^0-9A-Za-z]+", name)
    s = "".join(p[:1].upper() + p[1:] for p in parts if p)
    if not s or s[0].isdigit():
        s = "V" + s
    return "Self_" if s == "Self" else s


def catch_all(out):
    """Close an enum with its catch-all (`Unknown`, or `UnknownValue` when
    the protocol has an `unknown` value of its own)."""
    lines = "\n".join(out).splitlines()
    taken = any(
        ln.strip() == "Unknown," or ln.strip().startswith("Unknown(") for ln in lines
    )
    v = "UnknownValue" if taken else "Unknown"
    out.append("    /// A value this Codex version doesn't know.")
    out.append(f"    #[serde(untagged)]\n    {v}(Value),\n}}\n")
    return "\n".join(out) + "\n"


class Gen:
    def __init__(self, bundle):
        top = bundle["definitions"]
        self.defs = {k: v for k, v in top.items() if k != "v2"}
        self.defs.update(top["v2"])  # v2 wins on a shared name
        self.top = top
        self.items = {}  # name -> rust source
        self.order = []
        self.defaultable = {}
        self.pending = []

    def ref(self, r):
        name = r.split("/")[-1]
        if name not in self.items and name not in self.pending:
            self.pending.append(name)
        return name

    def drain(self):
        while self.pending:
            name = self.pending.pop(0)
            if name in self.items:
                continue
            self.items[name] = None  # reserve (cycles)
            self.order.append(name)
            if name in OPAQUE:
                self.items[name] = f"pub type {name} = Value;\n"
                self.defaultable[name] = True
            else:
                self.items[name] = self.define(name, self.defs[name])

    # A rust type for a schema used in a field (inline objects are named
    # after `hint`).
    def ty(self, s, hint):
        if s is True or s == {}:
            return "Value"
        if "$ref" in s:
            return self.ref(s["$ref"])
        for k in ("anyOf", "oneOf"):
            if k in s and "properties" not in s:
                alts = s[k]
                non_null = [a for a in alts if a.get("type") != "null"]
                if len(non_null) == 1 and len(non_null) < len(alts):
                    return f"Option<{self.ty(non_null[0], hint)}>"
                if k == "oneOf" or all("$ref" not in a for a in alts):
                    return self.inline(s, hint)
                return "Value"
        if "allOf" in s and len(s["allOf"]) == 1:
            return self.ty(s["allOf"][0], hint)
        t = s.get("type")
        if isinstance(t, list):
            rest = [x for x in t if x != "null"]
            inner = dict(s, type=rest[0]) if len(rest) == 1 else {}
            base = self.ty(inner, hint)
            return f"Option<{base}>" if "null" in t else base
        if t == "string":
            if "enum" in s and len(s["enum"]) > 1:
                return self.inline(s, hint)
            return "String"
        if t == "integer":
            return "u64" if str(s.get("format", "")).startswith("uint") else "i64"
        if t == "number":
            return "f64"
        if t == "boolean":
            return "bool"
        if t == "array":
            return f"Vec<{self.ty(s.get('items', {}), hint + 'Item')}>"
        if t == "object":
            if s.get("properties"):
                return self.inline(s, hint)
            ap = s.get("additionalProperties", True)
            v = "Value" if ap is True or ap is False else self.ty(ap, hint + "Value")
            return f"BTreeMap<String, {v}>"
        return "Value"

    def inline(self, s, hint):
        name = pascal(hint)
        while name in self.items or name in self.defs:
            name += "_"
        self.items[name] = None
        self.order.append(name)
        self.items[name] = self.define(name, s)
        return name

    def define(self, name, s):
        doc = doc_lines(s.get("description"))
        if "oneOf" in s and "properties" not in s:
            return doc + self.one_of(name, s["oneOf"])
        if s.get("type") == "string" and "enum" in s:
            self.defaultable[name] = False
            return doc + self.enum(name, [(v, None) for v in s["enum"]])
        if "properties" in s:
            return doc + self.struct(name, s)
        self.defaultable[name] = True
        return doc + f"pub type {name} = {self.ty(s, name + 'Inner')};\n"

    def enum(self, name, values):
        out = [
            "#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]",
            f"pub enum {name} {{",
        ]
        seen = set()
        for v, d in values:
            vn = pascal(v)
            while vn in seen:
                vn += "_"
            seen.add(vn)
            out += doc_lines(d, "    ").splitlines()
            out.append(f'    #[serde(rename = "{v}")]\n    {vn},')
        return catch_all(out)

    def one_of(self, name, alts):
        self.defaultable[name] = False
        if all(a.get("type") == "string" and "enum" in a for a in alts):
            vals = [(v, a.get("description")) for a in alts for v in a["enum"]]
            return self.enum(name, vals)
        tag = all(
            a.get("type") == "object"
            and "enum" in a.get("properties", {}).get("type", {})
            and len(a["properties"]["type"]["enum"]) == 1
            for a in alts
        )
        out = ["#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]"]
        if tag:
            out.append('#[serde(tag = "type")]')
        out.append(f"pub enum {name} {{")
        for a in alts:
            if tag:
                v = a["properties"]["type"]["enum"][0]
                body = dict(
                    a,
                    properties={
                        k: p for k, p in a["properties"].items() if k != "type"
                    },
                )
                sname = a.get("title") or (name + pascal(v))
                if sname in self.defs or sname in self.items:
                    sname = name + pascal(v)
                self.items[sname] = None
                self.order.append(sname)
                self.items[sname] = self.struct(sname, body)
                out += doc_lines(a.get("description"), "    ").splitlines()
                out.append(f'    #[serde(rename = "{v}")]\n    {pascal(v)}({sname}),')
            elif a.get("type") == "string" and "enum" in a:
                for v in a["enum"]:
                    out += doc_lines(a.get("description"), "    ").splitlines()
                    out.append(f'    #[serde(rename = "{v}")]\n    {pascal(v)},')
            elif (
                a.get("type") == "object"
                and len(a.get("required", [])) == 1
                and len(a.get("properties", {})) == 1
            ):
                key = a["required"][0]
                inner = self.ty(a["properties"][key], name + pascal(key))
                out += doc_lines(a.get("description"), "    ").splitlines()
                out.append(
                    f'    #[serde(rename = "{key}")]\n    {pascal(key)}({inner}),'
                )
        return catch_all(out)

    def struct(self, name, s):
        req = set(s.get("required", []))
        props = dict(s.get("properties", {}))
        # Properties of `anyOf`/`oneOf` alternatives (one of several
        # shapes) are each optional.
        for k in ("anyOf", "oneOf"):
            for a in s.get(k, []):
                for pk, pv in a.get("properties", {}).items():
                    props.setdefault(pk, pv)
        fields = []
        defaultable = True
        for pk in sorted(props):
            ps = props[pk]
            t = self.ty(ps, name + pascal(pk))
            opt = pk not in req and not t.startswith("Option<")
            if opt:
                t = f"Option<{t}>"
            if t.startswith("Option<"):
                attr = '#[serde(default, skip_serializing_if = "Option::is_none")]'
            else:
                attr = None
                base = t.split("<")[0]
                if base not in (
                    "Vec",
                    "BTreeMap",
                    "Value",
                    "String",
                    "i64",
                    "u64",
                    "f64",
                    "bool",
                ) and not self.defaultable.get(base, False):
                    defaultable = False
            fields.append(
                (pk, t, attr, ps.get("description") if isinstance(ps, dict) else None)
            )
        self.defaultable[name] = defaultable
        derive = "Debug, Clone, PartialEq, Serialize, Deserialize" + (
            ", Default" if defaultable else ""
        )
        out = [f"#[derive({derive})]", f"pub struct {name} {{"]
        for pk, t, attr, d in fields:
            out += doc_lines(d, "    ").splitlines()
            fname = snake(pk)
            if fname.removeprefix("r#") != pk:
                out.append(f'    #[serde(rename = "{pk}")]')
            if attr:
                out.append(f"    {attr}")
            out.append(f"    pub {fname}: {t},")
        out.append("}\n")
        return "\n".join(out) + "\n"


def doc_lines(d, indent=""):
    if not d:
        return ""
    lines = d.strip().replace("\r", "").split("\n")
    return "".join(
        f"{indent}///{(' ' + ln.rstrip()) if ln.strip() else ''}\n" for ln in lines
    )


def methods(top, union):
    out = {}
    for a in top[union]["oneOf"]:
        m = a["properties"]["method"]["enum"][0]
        p = a["properties"].get("params", {}).get("$ref")
        out[m] = p.split("/")[-1] if p else None
    return out


def main():
    schema, version, out_path = sys.argv[1:4]
    bundle = json.load(open(schema))
    ver = open(version).read().strip()
    g = Gen(bundle)
    top = bundle["definitions"]
    impls = []
    creq = methods(top, "ClientRequest")
    for m in REQUESTS:
        p = creq[m]
        r = p.removesuffix("Params") + "Response"
        g.ref(p)
        g.ref(r)
        impls.append(
            f'impl Request for {p} {{\n    const METHOD: &\'static str = "{m}";\n    type Response = {r};\n}}\n'
        )
    notes = methods(top, "ServerNotification")
    for m in NOTIFICATIONS:
        p = notes[m]
        g.ref(p)
        impls.append(
            f'impl Notification for {p} {{\n    const METHOD: &\'static str = "{m}";\n}}\n'
        )
    sreq = methods(top, "ServerRequest")
    for m in SERVER_REQUESTS:
        p = sreq[m]
        r = p.removesuffix("Params") + "Response"
        g.ref(p)
        g.ref(r)
        impls.append(
            f'impl ServerRequest for {p} {{\n    const METHOD: &\'static str = "{m}";\n    type Response = {r};\n}}\n'
        )
    g.drain()
    head = f"""// @generated by scripts/gen-codex-protocol from
// schema/ (codex {ver}). Do not edit.

use alloc::collections::BTreeMap;

use serde::{{Deserialize, Serialize, de::DeserializeOwned}};
use serde_json::Value;

/// The Codex version this schema came from (`codex --version`).
pub const CODEX_VERSION: &str = "{ver}";

/// A client request: its method name and response type.
pub trait Request: Serialize {{
    const METHOD: &'static str;
    type Response: DeserializeOwned;
}}

/// A server notification: its method name.
pub trait Notification: DeserializeOwned {{
    const METHOD: &'static str;
}}

/// A server-to-client request: its method name and response type.
pub trait ServerRequest: DeserializeOwned {{
    const METHOD: &'static str;
    type Response: Serialize;
}}

"""
    body = "".join(impls) + "\n" + "\n".join(g.items[n] for n in sorted(g.order))
    with open(out_path, "w") as f:
        f.write(head + body)


if __name__ == "__main__":
    main()
