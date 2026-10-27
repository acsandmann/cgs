#include <metal_stdlib>
using namespace metal;
struct Params {
    float4x4 transform;
    float4 camera; // scale, translation x/y, reserved
    float4 rect;
    float4 clip;
    float4 viewport; // width, height, opacity, corner radius
    float4 uv;
    float4 border;
    float4 style;
};
struct Vertex { float4 position [[position]]; float2 uv; float2 local; float2 screen [[center_no_perspective]]; };
vertex Vertex preview_vertex(uint id [[vertex_id]], constant Params &p [[buffer(0)]]) {
    const float2 corners[] = {float2(0,0), float2(1,0), float2(0,1), float2(1,1)};
    float2 uv = corners[id];
    float4 pos = p.transform * float4(p.rect.xy + uv * p.rect.zw, 0, 1);
    pos.xy = pos.xy * p.camera.x + p.camera.yz * pos.w;
    Vertex v;
    v.position = float4(2 * pos.x / p.viewport.x - pos.w,
                       pos.w - 2 * pos.y / p.viewport.y, pos.z, pos.w);
    v.uv = uv; v.local = uv * p.rect.zw; v.screen = pos.xy / pos.w;
    return v;
}
fragment float4 preview_fragment(Vertex v [[stage_in]], constant Params &p [[buffer(0)]],
                                 texture2d<float> image [[texture(0)]], sampler filtering [[sampler(0)]]) {
    if (any(v.screen < p.clip.xy) || any(v.screen >= p.clip.zw)) discard_fragment();
    float r = min(p.viewport.w, min(p.rect.z, p.rect.w) * 0.5);
    float2 q = abs(v.local - p.rect.zw * 0.5) - p.rect.zw * 0.5 + r;
    float distance = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
    float coverage = 1.0 - smoothstep(-fwidth(distance)*0.5, fwidth(distance)*0.5, distance);
    float4 color = image.sample(filtering, p.uv.xy + v.uv * p.uv.zw);
    if (p.style.x > 0.0) {
        float borderCoverage = smoothstep(-p.style.x - fwidth(distance)*0.5, -p.style.x + fwidth(distance)*0.5, distance);
        float4 border = p.border * borderCoverage;
        color = border + color * (1.0 - border.a);
    }
    return color * (p.viewport.z * coverage);
}
