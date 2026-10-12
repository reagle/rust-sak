/// The angular frequency, in radians per second.
const FREQUENCY: f64 = 18.0;
/// The damping ratio; `1` is critically damped, so the spring settles without oscillating.
const DAMPING: f64 = 1.0;
/// The time step: one frame of the display.
#[allow(
    clippy::cast_precision_loss,
    reason = "the frame rate is a small integer, exact as an `f64`"
)]
const STEP: f64 = 1.0 / super::FPS as f64;

/// Within these distances of the target and of rest, the spring snaps to the target.
const POSITION_EPSILON: f64 = 0.001;
const VELOCITY_EPSILON: f64 = 0.01;

/// A critically damped spring that eases a value (such as a bar's fill) towards its target, one frame at a time.
///
/// Set [`target`](Self::target), then call [`step`](Self::step) once per frame and draw [`position`](Self::position).
/// From rest, it covers the whole `0` to `1` range in under a second and never overshoots.
#[derive(Debug, Default, Clone, Copy)]
pub struct Spring {
    /// Where the spring is now.
    pub position: f64,
    velocity: f64,
    /// Where the spring is heading.
    pub target: f64,
}

impl Spring {
    /// Advances the spring by one frame, with semi-implicit Euler.
    pub fn step(&mut self) {
        if self.is_settled() {
            return;
        }

        let acceleration =
            -FREQUENCY * FREQUENCY * (self.position - self.target) - 2.0 * DAMPING * FREQUENCY * self.velocity;
        self.velocity += acceleration * STEP;
        self.position += self.velocity * STEP;

        if (self.target - self.position).abs() < POSITION_EPSILON && self.velocity.abs() < VELOCITY_EPSILON {
            self.snap();
        }
    }

    /// Whether the spring rests on its target.
    #[allow(clippy::float_cmp, reason = "`snap` copies the target exactly, so equality is exact")]
    pub fn is_settled(&self) -> bool {
        self.position == self.target && self.velocity == 0.0
    }

    /// Moves the spring onto its target at rest.
    pub fn snap(&mut self) {
        self.position = self.target;
        self.velocity = 0.0;
    }
}
