in vec4 a_motion; // origin.xy, launch velocity.xy
in vec4 a_shape;  // radius, birth time, color seed, kind
uniform vec4 u_viewport;
uniform float u_time;
out vec2 v_uv;
out float v_age;
out float v_seed;
out float v_kind;
const vec2 corners[6] = vec2[6](vec2(-1.,-1.),vec2(1.,-1.),vec2(-1.,1.),vec2(-1.,1.),vec2(1.,-1.),vec2(1.,1.));
void main() {
    float age = max(0., u_time - a_shape.y);
    float life = clamp(1. - age / PARTICLE_LIFETIME, 0., 1.);
    vec2 uv = corners[gl_VertexID];
    float drag = 2.8;
    float travel = (1. - exp(-drag * age)) / drag;
    vec2 gravity = vec2(0., 220.);
    vec2 center = a_motion.xy + a_motion.zw * travel + gravity * age * age;
    vec2 velocity = a_motion.zw * exp(-drag * age) + gravity * 2. * age;
    vec2 direction = normalize(velocity + vec2(.0001, 0.));
    vec2 normal = vec2(-direction.y, direction.x);
    float tail = length(velocity) * .018 * life;
    vec2 extent = vec2(a_shape.x + tail, max(.75, a_shape.x * .65));
    center -= direction * tail * .45;
    vec2 offset = direction * uv.x * extent.x + normal * uv.y * extent.y;
    if (a_shape.w > 2.5) {
        // A few dim, expanding puffs follow the initial kick, then rise.
        center = a_motion.xy + a_motion.zw * travel * .12 - vec2(0., age * 20.);
        offset = uv * (3. + age * 15.) * (1. + a_shape.z * .4);
    } else if (a_shape.w > 1.5) {
        // Crisp tumbling fragments, with the same ballistic gravity as sparks.
        float angle = a_shape.z * 6.283185 + age * (a_shape.z - .5) * 18.;
        vec2 rotated = vec2(uv.x * cos(angle) - uv.y * sin(angle), uv.x * sin(angle) + uv.y * cos(angle));
        offset = rotated * a_shape.x * (.75 + life * .25);
    } else if (a_shape.w > .5) {
        center = a_motion.xy;
        offset = uv * a_shape.x;
    }
    vec2 point = (center + offset - u_viewport.xy) / u_viewport.zw;
    gl_Position = vec4(point.x * 2. - 1., 1. - point.y * 2., 0., 1.);
    if (life <= 0. || (a_shape.w > .5 && a_shape.w < 1.5 && age >= .12)) {
        gl_Position = vec4(2., 2., 0., 1.);
    }
    v_uv = uv;
    v_age = age;
    v_seed = a_shape.z;
    v_kind = a_shape.w;
}
