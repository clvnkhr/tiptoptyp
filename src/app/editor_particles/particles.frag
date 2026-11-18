in vec2 v_uv;
in float v_age;
in float v_seed;
in float v_kind;
flat in float v_style;
uniform vec3 u_color;
uniform float u_dark;
out vec4 out_color;
float lifetime(float style) {
    if (style == 5.) return SMOKE_LIFETIME;
    if (style == 6.) return EMBER_LIFETIME;
    if (style == 7.) return AFTERBURNER_LIFETIME;
    return PARTICLE_LIFETIME;
}
void main() {
    float life = clamp(1. - v_age / lifetime(v_style), 0., 1.);
    float radius = length(v_uv);
    vec3 fire = mix(vec3(1., .28, .035), vec3(1., .86, .32), v_seed);
    vec3 ember = mix(vec3(1., .88, .30), vec3(1., .24, .04), smoothstep(.1, .75, 1. - life));
    vec3 color = mix(ember, u_color, step(.65, v_seed) * .75);
    float alpha;
    if (v_style == 5.) {
        // Three overlapping polynomial puffs, without textures or per-pixel
        // noise. Low opacity keeps the source readable beneath lingering smoke.
        vec2 lower = (v_uv + vec2(.22, .12)) * vec2(1.15, 1.25);
        vec2 upper = (v_uv - vec2(.2, -.22)) * vec2(1.45, 1.5);
        float puff = max(0., 1. - dot(v_uv * vec2(1., 1.1), v_uv * vec2(1., 1.1)));
        float lobe = max(0., 1. - dot(lower, lower));
        float crown = max(0., 1. - dot(upper, upper));
        float cloud = max(puff * puff, max(lobe * lobe * .8, crown * crown * .7));
        alpha = cloud * .12 * life * life * smoothstep(0., .14, v_age);
        color = mix(vec3(.24 + .12 * u_dark), u_color, .25);
    } else if (v_style == 6.) {
        vec2 core_uv = v_uv * vec2(.8, 1.25);
        float core = exp(-dot(core_uv, core_uv) * 3.5);
        alpha = core * life * life;
        color = mix(mix(vec3(1., .65, .2), vec3(.68, .1, .025), 1. - life), u_color, .25);
        color = mix(color, vec3(1.), core * life * .4 * u_dark);
    } else if (v_style == 7.) {
        vec2 plume_uv = (v_uv - vec2(.1, 0.)) * vec2(.9, 1.45);
        float plume = exp(-dot(plume_uv, plume_uv) * 2.5);
        vec2 hot_uv = (v_uv - vec2(.48, 0.)) * vec2(2.4, 2.);
        float core = exp(-dot(hot_uv, hot_uv) * 3.);
        alpha = min(1., plume * .8 + core * .5) * life * life;
        color = mix(mix(vec3(1., .22, .035), u_color, .35), vec3(1., .95, .72), core * life);
    } else if (v_style == 2. && v_kind < .5) {
        // Expanding flame tongues carry a hot yellow-white core into a cooling
        // orange/ember envelope; the staggered lobes avoid uniform orange dots.
        vec2 body_uv = (v_uv + vec2(.18, 0.)) * vec2(.8, 1.35);
        float flame = exp(-dot(body_uv, body_uv) * 2.8);
        vec2 tongue_uv = (v_uv - vec2(.4, (v_seed - .5) * .45)) * vec2(1.8, 2.);
        float tongue = max(0., 1. - dot(tongue_uv, tongue_uv));
        float core = tongue * tongue;
        alpha = min(1., flame * .7 + core * .45) * life * .9;
        vec3 cooling = mix(vec3(.72, .055, .012), vec3(1., .48, .045), life);
        color = mix(mix(cooling, u_color, .12), vec3(1., .94, .58), core * life);
    } else if (v_kind > 2.5) {
        float puff = exp(-radius * radius * 3.5) * (1. - smoothstep(.75, 1., radius));
        alpha = puff * .14 * life * life * smoothstep(0., .05, v_age);
        color = mix(vec3(.35 - .2 * u_dark), u_color, .25);
    } else if (v_kind > 1.5) {
        alpha = (1. - smoothstep(.72, 1., max(abs(v_uv.x), abs(v_uv.y)))) * life;
        color = mix(color, fire * .55, .3);
    } else if (v_kind > .5) {
        float fade = pow(clamp(1. - v_age / .12, 0., 1.), 2.);
        float angle = atan(v_uv.y, v_uv.x + .0001);
        float edge = .35 + .12 * sin(angle * 5. + v_seed * 9.) + .08 * cos(angle * 9.);
        float flame = 1. - smoothstep(edge * .5, edge, radius);
        float core = exp(-radius * radius * 45.);
        alpha = min(1., flame * .6 + core) * fade;
        color = mix(mix(u_color, fire, .8), vec3(1.), core * u_dark);
    } else {
        vec2 head_uv = (v_uv - vec2(.55, 0.)) * vec2(3., 1.8);
        float head = exp(-dot(head_uv, head_uv) * 2.);
        float tail = (1. - smoothstep(.12, .7, abs(v_uv.y)))
            * smoothstep(-1., -.55, v_uv.x) * (1. - smoothstep(.45, 1., v_uv.x));
        alpha = min(1., head + tail * .55) * pow(life, 1.2);
        color = mix(color, vec3(1.), head * life * .8 * u_dark);
    }
    if (v_style == 1.) {
        color = mix(color, mix(vec3(.8, .58, .26), u_color, .3), .4);
    } else if (v_style == 3.) {
        color = mix(vec3(.05, .75, 1.), u_color, .45);
        if (v_kind > .5 && v_kind < 1.5) {
            alpha = exp(-v_uv.y * v_uv.y * 9.) * (1. - smoothstep(.65, 1., abs(v_uv.x)))
                  * pow(clamp(1. - v_age / .12, 0., 1.), 1.5);
        }
    } else if (v_style == 4.) {
        color = mix(mix(vec3(.46, .12, 1.), vec3(.05, 1., .8), v_seed), u_color, .35);
        if (v_kind < .5) {
            float core = exp(-radius * radius * 10.);
            alpha = (core + exp(-radius * radius * 3.) * .3) * life;
            color = mix(color, vec3(1.), core * .5 * u_dark);
        } else if (v_kind < 1.5) {
            float ring = (radius - .65) * 12.;
            alpha = exp(-ring * ring) * pow(clamp(1. - v_age / .24, 0., 1.), 1.5);
        }
    }
    alpha = clamp(alpha, 0., 1.);
    if (alpha < .003) discard;
    out_color = vec4(color * alpha, alpha);
}
