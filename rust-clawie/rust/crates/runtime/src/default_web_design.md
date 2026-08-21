# Clawie default web design system

Use this original baseline when a project does not supply its own `DESIGN.md`. It combines an open Awesome DESIGN.md-inspired token baseline with the applicable landing-page practices from Taste Skill. Do not copy a source brand, logo, or proprietary identity.

## Choose a direction before writing UI

- **Dashboard default:** a quiet, dense work surface. Use a neutral or near-black canvas, subtle borders, one restrained accent, readable tables, filters, tabs, status chips, and real empty/loading/error states. Prioritize scanning, hierarchy, and operational controls over oversized cards or decoration.
- **Landing-page default:** a clear editorial product story. First viewport: concise value proposition, one primary CTA, optional secondary CTA, and a real interface preview or concrete visual proof. Follow with trust, workflow/features, proof, and a focused closing CTA. Use large type and generous space only where they sharpen the story.
- **Visual product default:** choose one coherent thesis (developer tool, data platform, premium consumer, creative tool, or editorial product) and keep typography, contrast, imagery, and motion consistent with it.

## Clawie marketing taste protocol

Apply this protocol to landing pages, portfolios, editorial pages, and redesigns. Do not apply its marketing layout rules to dense dashboards, tables, wizards, or admin software. For those, use the dashboard default and an appropriate established component system.

1. Start with a one-line **Design Read**: page kind, audience, brand or vibe, and the chosen design-system or aesthetic family. If the direction genuinely cannot be inferred, ask one focused question before building.
2. Set three intentional dials before choosing layout: `DESIGN_VARIANCE` (symmetry to asymmetry), `MOTION_INTENSITY` (static to cinematic), and `VISUAL_DENSITY` (airy to compact). For a typical SaaS landing page, begin at 7 / 6 / 4, then adjust from the brief. Do not silently use a generic visual default.
3. Use an official design system when the product clearly calls for one. Examples: Fluent for Microsoft-like enterprise apps, Carbon for IBM-like analytics, Polaris for Shopify apps, Atlaskit for Atlassian-like products, and GOV.UK or USWDS for public service. Use one system per project, check existing dependencies first, and do not recreate official components by hand.
4. For an aesthetic rather than a system, build an original, coherent language. Do not claim that an approximation is an official implementation.

## Anti-slop rules for marketing surfaces

- Do not default to an AI-purple glow, centered hero over a mesh gradient, three equal feature cards, blanket glassmorphism, repeated eyebrow labels, fake version stamps, decorative status dots, or poetic filler copy.
- Prefer an asymmetric or left-aligned hero when variance is above 4. A centered hero is reserved for an editorial manifesto or a launch where the message itself is the visual.
- Hero discipline: fit the first viewport, keep the headline to two desktop lines, use at most one eyebrow or brand strip, one concise supporting paragraph, and one primary action with an optional secondary action. Put trust logos below the hero, never inside it.
- Use one accent, one neutral family, one radius system, and one theme strategy. Do not mix warm and cool gray systems, invert arbitrary sections, or use cards where spacing and dividers communicate hierarchy better.
- Choose a deliberate sans display face or project font. Do not default to Inter, generic serif emphasis, or Fraunces/Instrument Serif. Use mono only for code, metrics, and technical labels.
- Use real product screenshots, supplied assets, image generation, or a working component preview. Do not build a fake dashboard or terminal from decorative rectangles. If no suitable visual is available, leave a clear asset placeholder instead of inventing one.
- Use concrete copy and real data. Avoid invented precision, generic people and company names, empty superlatives, filler verbs, and duplicate CTA intent.
- No em dashes in visible marketing copy. Use short sentences, commas, colons, or regular hyphens instead.

## Interaction, accessibility, and delivery

- Motion must communicate hierarchy, storytelling, feedback, or state. Honor reduced motion, animate transform and opacity only, and use Motion, ScrollTrigger, IntersectionObserver, or CSS scroll-driven animations instead of scroll listeners that update React state.
- Design responsive behavior and the hover, focus, active, disabled, empty, loading, error, and success states. Keep navigation on one desktop line, make mobile collapse explicit, and avoid `h-screen` in favor of stable dynamic viewport sizing.
- Before delivery, check contrast, text fit, CTA wrapping, layout repetition, real asset use, responsive behavior, keyboard focus, reduced motion, image dimensions, and layout shift. Test both color modes when the product is consumer-facing or supports them.

## Tokens and composition

- Typography: use a deliberate modern sans (`Geist`, `Satoshi`, `Cabinet Grotesk`, or a project-appropriate system fallback); reserve a mono face for code, metrics, and technical labels. Make headings tight and deliberate, body copy comfortable, and utility labels compact.
- Color: use neutral surfaces plus one primary accent. Keep semantic success, warning, and error distinct. Bright gradients belong only to a purposeful hero or data visual, never as generic background filler.
- Spacing: use a 4px/8px rhythm (4, 8, 12, 16, 24, 32, 48, 64, 96). Keep page sections spacious; keep product controls compact.
- Shape: use modest radii (4px–12px) and hairline borders. Avoid defaulting everything to pills, heavy shadows, glass effects, or floating cards.
- Content: prefer real interface states, meaningful labels, concrete metrics, and product-specific visuals over lorem ipsum, empty gradients, or stock decoration.

## Quality bar

- Design responsive behavior, keyboard focus, hover/active/disabled states, and empty/loading/error/success states as part of the implementation.
- Check hierarchy, text fit, contrast, touch targets, overflow, and stable layout at mobile and desktop widths before calling the UI complete.
- If the user asks for a specific visual reference, use it as the direction. If a local `DESIGN.md` exists, it overrides this baseline completely where they conflict.
