in vec4 a_motion; // origin.xy, launch velocity.xy
in vec4 a_shape;  // radius, birth time, color seed, packed style/kind
uniform vec4 u_viewport;
uniform float u_time;
out vec2 v_uv;
out float v_age;
out float v_seed;
out float v_kind;
flat out float v_style;
const vec2 corners[PARTICLE_VERTEX_COUNT] = vec2[PARTICLE_VERTEX_COUNT](vec2(-1.,-1.),vec2(1.,-1.),vec2(-1.,1.),vec2(1.,1.));
void main() {
    float age = max(0., u_time - a_shape.y);
    float life = clamp(1. - age / PARTICLE_LIFETIME, 0., 1.);
    float kind = mod(a_shape.w, 4.);
    float style = floor(a_shape.w * .25);
    vec2 uv = corners[gl_VertexID];
    float drag = 2.8;
    vec2 gravity = vec2(0., 220.);
    if (style == 2.) { drag = 1.5; gravity.y = -45.; }
    else if (style == 3.) { drag = 4.5; gravity.y = 0.; }
    else if (style == 4.) { drag = 3.2; gravity.y = 0.; }
    float travel = (1. - exp(-drag * age)) / drag;
    vec2 displacement = a_motion.zw * travel;
    if (style == 4.) {
        float turn = age * (3. + a_shape.z * 3.);
        displacement = vec2(displacement.x * cos(turn) - displacement.y * sin(turn),
                            displacement.x * sin(turn) + displacement.y * cos(turn));
    }
    vec2 center = a_motion.xy + displacement + gravity * age * age;
    if (style == 2.) center.x += sin(age * 15. + a_shape.z * 6.28) * age * 18.;
    vec2 velocity = a_motion.zw * exp(-drag * age) + gravity * 2. * age;
    vec2 direction = normalize(velocity + vec2(.0001, 0.));
    vec2 normal = vec2(-direction.y, direction.x);
    float tail = length(velocity) * (style == 3. ? .045 : .018) * life;
    vec2 extent = vec2(a_shape.x + tail, max(.75, a_shape.x * .65));
    center -= direction * tail * .45;
    vec2 offset = direction * uv.x * extent.x + normal * uv.y * extent.y;
    if (kind > 2.5) {
        center = a_motion.xy + a_motion.zw * travel * .12 - vec2(0., age * 20.);
        offset = uv * (3. + age * 15.) * (1. + a_shape.z * .4);
    } else if (kind > 1.5) {
        float angle = a_shape.z * 6.283185 + age * (a_shape.z - .5) * 18.;
        vec2 rotated = vec2(uv.x * cos(angle) - uv.y * sin(angle), uv.x * sin(angle) + uv.y * cos(angle));
        offset = rotated * a_shape.x * (.75 + life * .25);
    } else if (kind > .5) {
        center = a_motion.xy;
        offset = uv * a_shape.x;
        if (style == 1.) { center.x += 9.; offset *= vec2(1.7, .6); }
        else if (style == 2.) { center.y -= 6.; offset *= vec2(.65, 1.2); }
        else if (style == 3.) { center.x += 66.; offset *= vec2(4.8, .16); }
        else if (style == 4.) offset *= 1. + age * 7.;
    } else if (style == 2.) {
        offset = direction * uv.x * (a_shape.x * (1. + age * 4.) + tail * .4)
               + normal * uv.y * a_shape.x * (1. + age * 3.);
    } else if (style == 4.) {
        offset = uv * a_shape.x * (1.2 + life * .8);
    }
    vec2 point = (center + offset - u_viewport.xy) / u_viewport.zw;
    gl_Position = vec4(point.x * 2. - 1., 1. - point.y * 2., 0., 1.);
    float flash_life = style == 4. ? .24 : .12;
    if (life <= 0. || (kind > .5 && kind < 1.5 && age >= flash_life)) {
        gl_Position = vec4(2., 2., 0., 1.);
    }
    v_uv = uv;
    v_age = age;
    v_seed = a_shape.z;
    v_kind = kind;
    v_style = style;
}
