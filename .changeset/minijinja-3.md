---
oakum: major
---

### Changed

Templates render with minijinja 3, which follows Jinja2 more closely, so a template you write can render differently or fail where it rendered before. The main differences: a printed list or map uses single quotes (`['a', 'b']`), and `zip` and `items` give tuples that print as `(1, 3)`; `round` rounds half to even (`2.5` gives `2.0`); `title` leaves the letter after an apostrophe lowercase (`Don't`); `max` and `min` compare strings ignoring case; a boolean passes `is number`; unary minus binds as in Jinja2 (`-a.b` negates `a.b`); dividing by zero and `format` with an undefined value fail the render instead of printing `inf` or nothing. An undefined variable's error names the expression, such as `` `name` is undefined ``, before oakum's list of what the template receives. Built-in templates render the same. minijinja's [3.0.0 release notes](https://github.com/mitsuhiko/minijinja/releases/tag/3.0.0) list every change.
