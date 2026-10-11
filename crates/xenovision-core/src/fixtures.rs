//! Species fixture builders (design doc §2.3, §8). File-based packaging
//! (loading/saving these as separate user-editable files, §8.3) lives in
//! `fixture_library`, built on top of the plain in-memory `CurveSet`
//! values this module returns.
//!
//! Every fixture is a single receptor pool (`curves`) plus one or more
//! colorspace regimes (`regimes`). A single-regime species declares its
//! one colorspace (e.g. "Trichromat"); the frog - whose visual system
//! genuinely spans three regimes (scotopic/mesopic/photopic, §2.3.10) -
//! declares all three in one fixture, replacing the old two-file
//! scotopic/photopic split; mantis shrimp declares none (§10.2).

use crate::blackbody;
use crate::curve::{CurveType, QuantityKind, SpectralCurve};
use crate::curve_set::{CurveSet, OpponentContrast, Regime};
use crate::govardovskii::{self, Chromophore};
use crate::oil_droplet;

const WL_MIN: f64 = 300.0;
const WL_MAX: f64 = 750.0;
const STEP: f64 = 5.0;

/// Regime name used by the single-regime species fixtures (every
/// "plain one colorspace" species in this app gets this one regime; the
/// frog's three are special-cased in `frog()`).
const SINGLE_COLOR_VISION_REGIMES: [&str; 4] = [
    "Trichromat",
    "Dichromat",
    "Tetrachromat",
    "Color vision tetrachromat",
];

fn cone_curve(name: &str, lambda_max: f64, citation: &str) -> SpectralCurve {
    cone_curve_with(Chromophore::A1, name, lambda_max, citation)
}

fn cone_curve_with(
    chromophore: Chromophore,
    name: &str,
    lambda_max: f64,
    citation: &str,
) -> SpectralCurve {
    let mut curve = SpectralCurve::new(name, CurveType::Sensitivity)
        .with_points(govardovskii::generate_points_with(
            chromophore,
            lambda_max,
            WL_MIN,
            WL_MAX,
            STEP,
        ))
        .with_quantity(QuantityKind::Sensitivity);
    curve
        .metadata
        .insert("citation".to_string(), citation.to_string());
    curve
        .metadata
        .insert("lambda_max_nm".to_string(), lambda_max.to_string());
    curve
}

/// Marks a `CurveSet` built from a generic per-receptor fallback (no
/// species-specific literature opponent pairing or noise data located)
/// with a consistent metadata note, rather than leaving the absence
/// unexplained.
fn note_generic_fallback(set: &mut CurveSet) {
    set.metadata.insert(
        "opponent_pairing_note".to_string(),
        "no species-specific literature opponent mechanism located; uses the generic per-receptor fallback".to_string(),
    );
    set.metadata.insert(
        "noise_data_note".to_string(),
        "no species-specific omega/eta density data located; left blank".to_string(),
    );
}

/// Human (*Homo sapiens*) trichromat, §2.3.1: S/M/L cones at
/// 420/530/560nm. Pool order is [S, M, L]; one "Trichromat" regime over
/// all three.
///
/// Luminance weighting, Weber fraction, and relative density are all
/// illustrative placeholders, not precise literature citations for this
/// specific app - flagged as such in metadata, consistent with how the
/// design doc treats approximated values elsewhere (§2.3's "Note on
/// citation confidence"). They're real enough to exercise the L+M-
/// dominant luminance weighting (§2.3.1) and sanity-check the ΔS metric
/// (§3.3.2) against the right ballpark, not for scientific use.
pub fn human() -> CurveSet {
    let mut set = CurveSet::new("Human (Homo sapiens)");

    let mut s_cone = cone_curve("S-cone", 420.0, "Schnapf, Kraft & Baylor 1987");
    s_cone.luminance_weight = Some(0.0); // "near zero" per §2.3.1
    s_cone.omega = Some(0.05);
    s_cone.eta = Some(1.0); // S cones are the rarest of the three

    let mut m_cone = cone_curve("M-cone", 530.0, "Schnapf, Kraft & Baylor 1987");
    m_cone.luminance_weight = Some(0.4);
    m_cone.omega = Some(0.05);
    m_cone.eta = Some(16.0);

    let mut l_cone = cone_curve("L-cone", 560.0, "Schnapf, Kraft & Baylor 1987");
    l_cone.luminance_weight = Some(0.6); // L+M-dominant, L somewhat > M
    l_cone.omega = Some(0.05);
    l_cone.eta = Some(32.0);

    set.curves = vec![s_cone, m_cone, l_cone];

    // Literature-standard red-green and blue-yellow contrasts (§2.3.1),
    // exercised through real CurveSet data per §8.2's representability
    // requirement. `opponent_contrasts` lives on the regime - the
    // colorspace it defines - not the set (§2.2).
    //
    // The blue-yellow row is written yellow-positive, i.e. negated from the
    // textbook S - (M+L)/2, so the composite encoder (whose c2 grows towards
    // yellow) renders a Human perceptual image with blue/yellow the right way
    // round. Magnitude-only metrics such as ΔS are unaffected by the sign.
    set.regimes = vec![Regime::new(SINGLE_COLOR_VISION_REGIMES[0], vec![0, 1, 2])];
    set.regimes[0].opponent_contrasts = Some(vec![
        OpponentContrast {
            name: "L - M (red-green)".to_string(),
            weights: vec![0.0, -1.0, 1.0],
        },
        OpponentContrast {
            name: "(M+L)/2 - S (yellow-blue)".to_string(),
            weights: vec![-1.0, 0.5, 0.5],
        },
    ]);

    set.metadata.insert(
        "note".to_string(),
        "Curve shapes are Govardovskii A1 template approximations, not digitized data".to_string(),
    );
    set.metadata.insert(
        "luminance_weight_note".to_string(),
        "L+M-dominant weighting is an illustrative approximation (L:M = 0.6:0.4, S~0), not a precise literature citation".to_string(),
    );
    set.metadata.insert(
        "noise_data_note".to_string(),
        "omega/eta are illustrative placeholders sufficient to sanity-check the ΔS metric, not precise literature citations for human specifically".to_string(),
    );

    set
}

/// Dog (*Canis lupus familiaris*) dichromat, §2.3.2: S/L-M cones at
/// 432 (average of the reported 429-435nm range)/555nm. A functional
/// dichromat analogous to a human blue-yellow-only system; no species-
/// specific noise or opponent-pairing data located, so both use the
/// generic fallback.
pub fn dog() -> CurveSet {
    let mut set = CurveSet::new("Dog (Canis lupus familiaris)");
    set.curves = vec![
        cone_curve(
            "S-cone",
            432.0,
            "Neitz, Geist & Jacobs 1989; Jacobs et al. 1993",
        ),
        cone_curve(
            "L/M-cone",
            555.0,
            "Neitz, Geist & Jacobs 1989; Jacobs et al. 1993",
        ),
    ];
    set.regimes = vec![Regime::new(SINGLE_COLOR_VISION_REGIMES[1], vec![0, 1])];
    set.metadata.insert(
        "note".to_string(),
        "Functional dichromat, analogous to a human blue-yellow-only system".to_string(),
    );
    note_generic_fallback(&mut set);
    set
}

/// Cat (*Felis catus*) trichromat by measured mechanism, §2.3.3: three
/// photopic mechanisms at 450/500/556nm. The 500nm mechanism's
/// contribution to color discrimination is behaviorally debated in the
/// literature, but this is a disputed *interpretation* of the measured
/// data, not a retraction of the measurement itself - all three
/// receptors participate in both luminance and chroma here, same as
/// every other species in this app.
pub fn cat() -> CurveSet {
    let mut set = CurveSet::new("Cat (Felis catus)");
    set.curves = vec![
        cone_curve(
            "450nm mechanism",
            450.0,
            "Cat retinal ganglion cell spectral sensitivity studies",
        ),
        cone_curve(
            "500nm mechanism",
            500.0,
            "Cat retinal ganglion cell spectral sensitivity studies",
        ),
        cone_curve(
            "556nm mechanism",
            556.0,
            "Cat retinal ganglion cell spectral sensitivity studies",
        ),
    ];
    set.regimes = vec![Regime::new(SINGLE_COLOR_VISION_REGIMES[0], vec![0, 1, 2])];
    set.metadata.insert(
        "note".to_string(),
        "Cats are frequently characterized behaviorally as functional dichromats, with the \
         500nm mechanism's contribution to color discrimination specifically debated - this is \
         a disputed behavioral interpretation, not a retraction of the measured three-mechanism \
         finding. This fixture models the measured data: all three receptors participate."
            .to_string(),
    );
    note_generic_fallback(&mut set);
    set
}

/// Honeybee (*Apis mellifera*) trichromat, §2.3.4: UV/blue/green-L
/// receptors at 344/436/550nm (550 is the average of the reported
/// 544-556nm range). Unlike dog/cat, honeybee has literature-identified
/// noise data and opponent mechanisms - but the exact published numeric
/// values weren't locatable with confidence here, so the *structure*
/// (green/L-dominant luminance, green opposing UV+blue) is modeled
/// illustratively rather than left at the generic fallback, flagged
/// clearly rather than presented as precisely sourced.
pub fn honeybee() -> CurveSet {
    let mut set = CurveSet::new("Honeybee (Apis mellifera)");

    let mut uv = cone_curve("UV receptor", 344.0, "Menzel & Backhaus 1991");
    uv.omega = Some(0.05);
    uv.eta = Some(1.0);

    let mut blue = cone_curve("Blue receptor", 436.0, "Menzel & Backhaus 1991");
    blue.omega = Some(0.05);
    blue.eta = Some(2.0);

    let mut green = cone_curve("Green/L receptor", 550.0, "Menzel & Backhaus 1991");
    green.omega = Some(0.05);
    green.eta = Some(4.0); // most abundant per literature's L-dominant framing
    green.luminance_weight = Some(0.8); // explicit L-dominant override, not the integral default

    set.curves = vec![uv, blue, green];
    set.regimes = vec![Regime::new(SINGLE_COLOR_VISION_REGIMES[0], vec![0, 1, 2])];
    set.regimes[0].opponent_contrasts = Some(vec![
        OpponentContrast {
            name: "UV - Blue".to_string(),
            weights: vec![1.0, -1.0, 0.0],
        },
        OpponentContrast {
            name: "Green - (UV+Blue)/2".to_string(),
            weights: vec![-0.5, -0.5, 1.0],
        },
    ]);

    set.metadata.insert(
        "note".to_string(),
        "Receptor count/order: [UV, Blue, Green/L]. Luminance weighting is L-receptor-dominant \
         per Menzel & Backhaus 1991 (honeybees use the long-wavelength receptor for both \
         achromatic and colour vision)."
            .to_string(),
    );
    set.metadata.insert(
        "noise_data_note".to_string(),
        "omega/eta reflect the literature's qualitative structure (RNL model, Vorobyev & Osorio \
         1998) - green/L receptor most abundant/least noisy - but the exact published numeric \
         values weren't re-verified here; treat as illustrative, not precise citations."
            .to_string(),
    );
    set.metadata.insert(
        "opponent_pairing_note".to_string(),
        "Candidate RNL-model-style contrasts reflecting honeybee's documented green-vs-(UV+blue) \
         opponency structure; the exact published linear coefficients weren't re-verified here."
            .to_string(),
    );
    set
}

/// Pigeon (*Columba livia*) tetrachromat, §2.3.5: four single-cone
/// classes, each an opsin absorption curve composited with that cone's
/// oil droplet transmittance filter (§2.3.5 - oil droplets act as sharp
/// short-wavelength cut-offs, not a simple shifted lambda_max). The raw
/// opsin and droplet curves stay in the pool outside any colorspace
/// regime for inspection; only the 4 composited effective curves are
/// members of the "Tetrachromat" regime the pipeline faces.
pub fn pigeon() -> CurveSet {
    let mut set = CurveSet::new("Pigeon (Columba livia)");

    // (opsin name, opsin lambda_max, droplet name, droplet 50%-cutoff)
    let classes = [
        ("UV/violet (SWS1)", 409.0, "Transparent droplet", 200.0), // negligible filtering
        ("Short-wave (SWS2)", 456.0, "Greenish/C-type droplet", 430.0),
        ("Medium-wave (RH2)", 510.0, "Yellow droplet", 475.0),
        ("Long-wave (LWS)", 567.0, "Orange/red droplet", 580.0), // red-field value, representative default
    ];

    let mut effective_curves = Vec::new();
    let mut raw_opsins = Vec::new();
    let mut droplets = Vec::new();
    for (opsin_name, opsin_lmax, droplet_name, cutoff) in classes {
        let opsin = cone_curve(
            opsin_name,
            opsin_lmax,
            "Bowmaker 1977; Bowmaker et al. 1997",
        );
        let mut droplet = SpectralCurve::new(droplet_name, CurveType::Transmittance)
            .with_points(oil_droplet::generate_points(
                cutoff,
                oil_droplet::DEFAULT_STEEPNESS_NM,
                WL_MIN,
                WL_MAX,
                STEP,
            ))
            .with_quantity(QuantityKind::Transmittance);
        droplet
            .metadata
            .insert("citation".to_string(), "Martin & Muntz 1978".to_string());
        droplet
            .metadata
            .insert("cutoff_50pct_nm".to_string(), cutoff.to_string());

        let effective_points = oil_droplet::composite_sensitivity_points(&opsin, &droplet, STEP);
        let mut effective =
            SpectralCurve::new(format!("{opsin_name} (effective)"), CurveType::Sensitivity)
                .with_points(effective_points)
                .with_quantity(QuantityKind::Sensitivity);
        effective.metadata.insert(
            "derived".to_string(),
            format!("{opsin_name} opsin x {droplet_name} transmittance"),
        );

        effective_curves.push(effective);
        raw_opsins.push(opsin);
        droplets.push(droplet);
    }

    // Pool: 4 effective curves first (the regime members), then the 8
    // raw opsin + droplet curves outside any regime.
    let mut pool = effective_curves;
    for (opsin, droplet) in raw_opsins.into_iter().zip(droplets) {
        pool.push(opsin);
        pool.push(droplet);
    }
    set.curves = pool;
    set.regimes = vec![Regime::new(
        SINGLE_COLOR_VISION_REGIMES[2],
        vec![0, 1, 2, 3],
    )];

    set.metadata.insert(
        "note".to_string(),
        "Red-field droplet values used as the representative default (pigeons have two retinal \
         fields - red and yellow - with different droplet populations, not modeled separately). \
         The 4 composited effective curves are the Tetrachromat regime's members; the raw opsin \
         and droplet curves stay in the pool outside any colorspace regime for inspection."
            .to_string(),
    );
    note_generic_fallback(&mut set);
    set
}

/// Goldfish (*Carassius auratus*) tetrachromat, §2.3.6: UV/S/M/L
/// receptors at 356/455/530/625nm, generated from the **A2**
/// (porphyropsin) chromophore template specifically rather than the
/// default A1 - goldfish use vitamin-A2-based visual pigments.
pub fn goldfish() -> CurveSet {
    let mut set = CurveSet::new("Goldfish (Carassius auratus)");
    set.curves = vec![
        cone_curve_with(
            Chromophore::A2,
            "UV receptor",
            356.0,
            "Palacios, Goldsmith & Bernard 1998; Hárosi 1976",
        ),
        cone_curve_with(
            Chromophore::A2,
            "S/blue receptor",
            455.0,
            "Palacios, Goldsmith & Bernard 1998; Hárosi 1976",
        ),
        cone_curve_with(
            Chromophore::A2,
            "M/green receptor",
            530.0,
            "Palacios, Goldsmith & Bernard 1998; Hárosi 1976",
        ),
        cone_curve_with(
            Chromophore::A2,
            "L/red receptor",
            625.0,
            "Palacios, Goldsmith & Bernard 1998; Hárosi 1976",
        ),
    ];
    set.regimes = vec![Regime::new(
        SINGLE_COLOR_VISION_REGIMES[2],
        vec![0, 1, 2, 3],
    )];
    set.metadata.insert(
        "note".to_string(),
        "Curve shapes use the Govardovskii A2 (porphyropsin) template, not the default A1 - \
         goldfish use vitamin-A2-based visual pigments. The A1/A2 choice is a generation-time \
         detail, not a stored field."
            .to_string(),
    );
    note_generic_fallback(&mut set);
    set
}

/// Humboldt penguin (*Spheniscus humboldti*) trichromat, §2.3.7: violet/
/// S/L receptors at 403/450/543nm. Unlike most birds (avian
/// tetrachromats with a 4th UV and long-wave class), penguins have lost
/// both the UV-sensitive and long-wavelength-sensitive classes - an
/// evolutionary reduction, not a data gap, explaining why this is N=3
/// rather than pigeon's N=4.
pub fn penguin() -> CurveSet {
    let mut set = CurveSet::new("Penguin (Spheniscus humboldti)");
    set.curves = vec![
        cone_curve("Violet-sensitive (SWS1)", 403.0, "Bowmaker & Martin 1985"),
        cone_curve("S/blue receptor", 450.0, "Bowmaker & Martin 1985"),
        cone_curve("L/green receptor", 543.0, "Bowmaker & Martin 1985"),
    ];
    set.regimes = vec![Regime::new(SINGLE_COLOR_VISION_REGIMES[0], vec![0, 1, 2])];
    set.metadata.insert(
        "note".to_string(),
        "Penguins have lost the UV-sensitive and long-wavelength-sensitive cone classes most \
         birds retain - an evolutionary reduction (aquatic adaptation favoring blue-green \
         discrimination), not a data gap, which is why this is N=3 rather than a typical avian N=4."
            .to_string(),
    );
    note_generic_fallback(&mut set);
    set
}

/// Mouse (*Mus musculus*) dichromat, §2.3.8: S(UV)/M(green) cones at
/// 360/508nm. A behaviorally-confirmed (not just measured-pigment)
/// dichromat, though in practice restricted toward the upper visual
/// field by a dorsal-ventral density gradient this fixture - like
/// pigeon's two retinal fields - collapses to a single retina-wide
/// average by default.
pub fn mouse() -> CurveSet {
    let mut set = CurveSet::new("Mouse (Mus musculus)");
    set.curves = vec![
        cone_curve("S-cone (UV)", 360.0, "Jacobs, Neitz & Deegan 1991"),
        cone_curve("M-cone (green)", 508.0, "Jacobs, Neitz & Deegan 1991"),
    ];
    set.regimes = vec![Regime::new(SINGLE_COLOR_VISION_REGIMES[1], vec![0, 1])];
    set.metadata.insert(
        "note".to_string(),
        "Behaviorally-confirmed dichromatic discrimination, though a dorsal-ventral density \
         gradient means it's practically concentrated in the upper visual field - collapsed to a \
         single retina-wide average here, as pigeon's two retinal fields are. A substantial \
         fraction of mouse cones also coexpress both opsins within the same photoreceptor \
         (unlike dog's cleanly segregated dichromacy) - a biological detail that doesn't change \
         the two-receptor-type model structurally, since the pipeline operates on sensitivity \
         curves rather than individual cell identity."
            .to_string(),
    );
    note_generic_fallback(&mut set);
    set
}

/// Swallowtail butterfly (*Papilio xuthus*), §2.3.9: tetrachromat for
/// color vision (UV/blue/green/red at 360/460/520/600nm - the "Color
/// vision tetrachromat" regime's members), plus two further measured
/// receptor classes that participate in non-chromatic vision (motion
/// processing) rather than color opponency. The violet and broad-band
/// receptors stay in the pool *outside any colorspace regime* per
/// §2.2.4's exception-free "every colorspace receptor contributes to
/// both luminance and chroma" rule - demonstrating pool-wide
/// unaffiliated receptors rather than requiring an exception to that
/// rule.
pub fn butterfly() -> CurveSet {
    let mut set = CurveSet::new("Swallowtail Butterfly (Papilio xuthus)");
    set.curves = vec![
        cone_curve("UV", 360.0, "Arikawa et al. 1999; Koshitaka et al. 2008"),
        cone_curve("Blue", 460.0, "Arikawa et al. 1999; Koshitaka et al. 2008"),
        cone_curve("Green", 520.0, "Arikawa et al. 1999; Koshitaka et al. 2008"),
        cone_curve("Red", 600.0, "Arikawa et al. 1999; Koshitaka et al. 2008"),
    ];

    let mut violet = cone_curve("Violet (non-color-vision)", 400.0, "Arikawa et al. 1999");
    violet.metadata.insert(
        "note".to_string(),
        "A narrow-band receptor derived from the same opsin as the UV receptor, spectrally \
         narrowed by a UV-absorbing filtering pigment rather than a distinct opsin gene. \
         Approximated here with the standard template at the same lambda_max - the narrowing \
         itself isn't modeled, since this curve is outside any colorspace regime and not used \
         by the color-vision pipeline."
            .to_string(),
    );

    let mut broad_band =
        SpectralCurve::new("Broad-band (non-color-vision)", CurveType::Sensitivity)
            .with_points(vec![
                (350.0, 0.3),
                (400.0, 0.7),
                (450.0, 0.9),
                (550.0, 0.9),
                (650.0, 0.7),
                (700.0, 0.4),
            ])
            .with_quantity(QuantityKind::Sensitivity);
    broad_band.metadata.insert(
        "citation".to_string(),
        "Kawasaki, Kinoshita, Weckström & Arikawa 2015".to_string(),
    );
    broad_band.metadata.insert(
        "note".to_string(),
        "Spans a wide wavelength range rather than a narrow peak - implicated in motion \
         processing, with faster response dynamics than the color-vision receptors. Hand-authored \
         broad plateau shape (not Govardovskii-generated, which always peaks), illustrative."
            .to_string(),
    );

    set.curves.push(violet);
    set.curves.push(broad_band);
    set.regimes = vec![Regime::new(
        SINGLE_COLOR_VISION_REGIMES[3],
        vec![0, 1, 2, 3],
    )];

    set.metadata.insert(
        "note".to_string(),
        "The 4 color-vision receptors (UV/Blue/Green/Red) are the 'Color vision tetrachromat' \
         regime every color calculation operates on. Two further measured receptor classes \
         (violet, broad-band) are documented in the literature as participating in non-chromatic \
         vision rather than color opponency, and are kept in the pool outside any colorspace \
         regime for completeness - not part of the N=4 the color-vision pipeline sees, per \
         §2.2.4's exception-free all-colorspace-receptors-contribute rule."
            .to_string(),
    );
    note_generic_fallback(&mut set);
    set
}

/// Frog (*Rana* spp.), §2.3.10 - *one* three-regime fixture standing in
/// for the old two-file scotopic/photopic split. Pool order:
/// [Green rod (433), Red rod (502), Blue cone (431), Middle cone (502),
/// Red cone (580)].
///
/// Three structurally distinct regimes (§2.2), all driven by the
/// per-receptor operating windows (§2.2's gating) each class carries:
/// - **Scotopic** [rods]: dim-light, rod-based colorspace.
/// - **Mesopic** [all five]: rods *and* cones jointly - a real third
///   colorspace that peaks only in the overlap band where both receptor
///   classes' windows contain their seen irradiance, not a blend of the
///   other two regimes' outputs. Its activity is the product of its
///   members' gains (`regime_gain = ∏ g_i`), so it collapses (≈0)
///   outside that band.
/// - **Photopic** [cones]: bright-light, cone-based colorspace.
///
/// The windows overlap where rods' 3e-2 upper bound meets cones' 1e-2
/// lower bound - the mesopic band (~1e-2..3e-2 W/m² seen).
pub fn frog() -> CurveSet {
    let mut set = CurveSet::new("Frog (Rana spp.)");

    // Rods: operate at dim scotopic irradiances, gated off in bright
    // light (§2.2's operating window). Cones: require bright photopic
    // irradiances, no upper bound (receptor `saturation` caps the signal
    // instead). Windows are PER-CURVE here because the two classes have
    // opposite bounds - the set has no default. All units are objective
    // physical irradiance (W/m², treating luminant curves as spectral
    // irradiance in W/m²/nm) - deliberately not a human-photometric
    // measure, since the model is for the frog's own visual system.
    // Bounds are ILLUSTRATIVE, not calibrated photometric values.
    set.curves = vec![
        cone_curve(
            "Green rod",
            433.0,
            "Denton & Wyllie 1955; Yovanovich et al. 2017",
        )
        .with_illumination_window(Some(1e-6), Some(3e-2))
        .with_illumination_softness(0.5),
        cone_curve(
            "Red rod",
            502.0,
            "Denton & Wyllie 1955; Yovanovich et al. 2017",
        )
        .with_illumination_window(Some(1e-6), Some(3e-2))
        .with_illumination_softness(0.5),
        cone_curve(
            "Blue cone",
            431.0,
            "Koskelainen, Hemilä & Donner 1994; Liebman & Entine 1968",
        )
        .with_illumination_window(Some(1e-2), None)
        .with_illumination_softness(0.5),
        cone_curve(
            "Middle/green cone",
            502.0,
            "Koskelainen, Hemilä & Donner 1994; Liebman & Entine 1968",
        )
        .with_illumination_window(Some(1e-2), None)
        .with_illumination_softness(0.5),
        cone_curve(
            "Red cone",
            580.0,
            "Koskelainen, Hemilä & Donner 1994; Liebman & Entine 1968",
        )
        .with_illumination_window(Some(1e-2), None)
        .with_illumination_softness(0.5),
    ];

    set.regimes = vec![
        Regime::new("Scotopic", vec![0, 1]),
        Regime::new("Mesopic", vec![0, 1, 2, 3, 4]),
        Regime::new("Photopic", vec![2, 3, 4]),
    ];

    set.metadata.insert(
        "note".to_string(),
        "One three-regime fixture replacing the old separate scotopic/photopic files: the rods \
         (Scotopic), the cones (Photopic), and both jointly (Mesopic) are three structurally \
         distinct colorspaces, each with its own membership, dimensionality (1/4/2 chroma \
         channels), and operating windows. Regime activity is the product of the members' \
         gains, so the Mesopic regime is active only in the band (~1e-2..3e-2 W/m² seen) \
         where both rod and cone windows contain the light each receptor actually absorbs \
         (E_i = ∫S·I dλ, W/m²). Window bounds are illustrative W/m² values, not calibrated \
         photometric measurements."
            .to_string(),
    );
    set.metadata.insert(
        "contested_cone_note".to_string(),
        "The 502nm middle cone's status is contested: confirmed via microspectrophotometry in \
         some species/studies, reported absent/contentious in others (e.g. R. temporaria per the \
         Koskelainen lineage of work). This N=3 cones default is a documented assumption to \
         verify per species/study, not a settled fact - fork via Save As and remove this cone \
         (and its membership in the Mesopic/Photopic regimes) to model the 2-cone alternative."
            .to_string(),
    );
    note_generic_fallback(&mut set);
    set
}

/// Mantis shrimp (Stomatopoda, e.g. *Odontodactylus scyllarus*), §10.2:
/// the motivating real-world example for that TODO - a species with far
/// *more* photoreceptor classes than any other fixture here (12, vs.
/// pigeon/goldfish's 4), yet **none** of them belong to a colorspace
/// regime. Behavioral/electrophysiological work (notably Thoen et al.
/// 2014's "chunk" hypothesis) found mantis shrimp color *discrimination*
/// performance is markedly worse than a human trichromat's despite the
/// much larger receptor count - the leading explanation is that each
/// receptor class is read out largely independently (e.g. via rapid
/// eye-scanning/temporal comparison across the midband rows) rather
/// than combined through opponent-process channels the way vertebrate
/// color vision in every other fixture here works. This fixture models
/// that directly: `regimes` is **empty** - the first fixture with no
/// colorspace at all, exercising the no-regime path through the app
/// (every receptor reported individually, §10.1/§10.2 - no species-
/// specific code needed, consistent with §8.2).
pub fn mantis_shrimp() -> CurveSet {
    let mut set = CurveSet::new("Mantis Shrimp (Stomatopoda)");
    let classes = [
        ("UV1", 325.0),
        ("UV2", 350.0),
        ("UV3", 370.0),
        ("UV4", 395.0),
        ("Midband row 1", 440.0),
        ("Midband row 2", 475.0),
        ("Midband row 3", 500.0),
        ("Midband row 4", 525.0),
        ("Midband row 5", 550.0),
        ("Midband row 6", 575.0),
        ("Midband row 7", 600.0),
        ("Midband row 8", 625.0),
    ];
    set.curves = classes
        .iter()
        .map(|(name, lmax)| {
            cone_curve(
                name,
                *lmax,
                "Cronin & Marshall 1989; Marshall, Land & Cronin 2007; Thoen et al. 2014",
            )
        })
        .collect();
    set.regimes = Vec::new();

    set.metadata.insert(
        "note".to_string(),
        "12 photoreceptor classes (illustrative λmax values spanning UV to red - the headline \
         '12 types' figure is well-established, but these specific wavelengths weren't \
         re-verified digit-by-digit against primary microspectrophotometry data, same confidence \
         caveat as every other fixture's approximated values). None of the 12 belongs to any \
         colorspace regime: no species in this app has more raw photoreceptor diversity, and \
         none has less colorspace participation. Luminance and chroma are always exactly 0 \
         (nothing to weight or oppose) - the direct consequence of modeling the hypothesis that \
         these channels aren't combined into a human-style opponent colorspace at all. The \
         individual receptor activations/operating states (§10.1) are this fixture's only \
         meaningful output."
            .to_string(),
    );
    set.metadata.insert(
        "opponent_pairing_note".to_string(),
        "Not applicable: zero colorspace regimes means zero opponent contrasts by \
         construction, not an unmodeled literature mechanism."
            .to_string(),
    );
    set.metadata.insert(
        "noise_data_note".to_string(),
        "Not applicable: ΔS needs a colorspace regime, and there is none here by design."
            .to_string(),
    );
    set
}

/// All 11 fixtures this app ships with (human + 9 species/systems +
/// frog + mantis shrimp, the frog's old two files now merged into one
/// three-regime fixture) - the embedded pristine defaults
/// `fixture_library`'s "restore defaults" regenerates the user-visible
/// files from.
pub fn builtin_fixtures() -> Vec<CurveSet> {
    vec![
        human(),
        dog(),
        cat(),
        honeybee(),
        pigeon(),
        goldfish(),
        penguin(),
        mouse(),
        butterfly(),
        frog(),
        mantis_shrimp(),
    ]
}

/// Default daytime Earth solar illuminant (§5.4), used as the fallback
/// when §5.3's reflectance back-derivation isn't given an explicit
/// illuminant. Per the design doc's own suggestion, this is a 5778K
/// black-body curve rather than transcribed ASTM G173/CIE D65 tabulated
/// data ("A 5778K black-body curve approximates the Sun's photospheric
/// output reasonably well as a parametrically-generated alternative") -
/// avoiding another large hand-transcribed data table in favor of code
/// that's already written, tested, and auditable (`blackbody`).
pub fn default_solar_illuminant() -> SpectralCurve {
    let notches: Vec<blackbody::AbsorptionNotch> = blackbody::builtin_default_notches()
        .into_iter()
        .map(|n| n.notch)
        .collect();
    let mut curve = blackbody::generate_blackbody_curve(5778.0, 280.0, 2500.0, 2.0, &notches);
    curve.name = "Daytime Earth solar (5778K black body)".to_string();
    curve.metadata.insert(
        "note".to_string(),
        "5778K black-body approximation per §5.4, not digitized ASTM G173/CIE D65 data".to_string(),
    );
    curve
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a Pipeline for `set`'s first regime against a flat
    /// illuminant and checks it produces exactly `expected_chroma`
    /// chroma channels - the generic shape check every single-regime
    /// species fixture below needs (N receptors -> N-1 chroma channels),
    /// run through the pipeline rather than just inspecting the
    /// CurveSet's shape, so it also catches a fixture that fails to
    /// actually build (e.g. a curve with no points).
    fn assert_pipeline_chroma_count(set: CurveSet, expected_chroma: usize) {
        let illuminant = SpectralCurve::new("flat", CurveType::Illumination)
            .with_points(vec![(WL_MIN, 1.0), (WL_MAX, 1.0)]);
        let n_receptors = set.curves.len();
        let pipeline =
            crate::pipeline::Pipeline::build(set.default_regime_config(), &illuminant, 1.0);
        assert_eq!(pipeline.chroma_axis_count(), expected_chroma);
        assert!(
            n_receptors >= expected_chroma + 1 || n_receptors == 0,
            "N={n_receptors} should give N-1 chroma channels"
        );
    }

    /// Chroma count for one specific regime (multi-regime species like
    /// the frog), through the same generic build path.
    fn assert_regime_chroma_count(set: &CurveSet, regime_idx: usize, expected_chroma: usize) {
        let illuminant = SpectralCurve::new("flat", CurveType::Illumination)
            .with_points(vec![(WL_MIN, 1.0), (WL_MAX, 1.0)]);
        let pipeline = crate::pipeline::Pipeline::build(
            set.regime_config(&set.regimes[regime_idx]),
            &illuminant,
            1.0,
        );
        assert_eq!(pipeline.chroma_axis_count(), expected_chroma);
    }

    #[test]
    fn human_fixture_has_three_cones_in_s_m_l_order() {
        let set = human();
        assert_eq!(set.curves.len(), 3);
        assert_eq!(set.curves[0].name, "S-cone");
        assert_eq!(set.curves[1].name, "M-cone");
        assert_eq!(set.curves[2].name, "L-cone");
        assert_eq!(set.regimes.len(), 1, "single colorspace regime");
        assert_eq!(set.regimes[0].name, "Trichromat");
        assert_eq!(set.regimes[0].curves, vec![0, 1, 2]);
    }

    #[test]
    fn human_fixture_round_trips_through_json() {
        let set = human();
        let json = serde_json::to_string(&set).unwrap();
        let back: CurveSet = serde_json::from_str(&json).unwrap();
        assert_eq!(set, back);
    }

    #[test]
    fn human_fixture_has_complete_noise_data_and_opponent_contrasts() {
        let set = human();
        assert!(!set.has_partial_eta_coverage());
        assert!(set.receptor_noise().is_some());
        let contrasts = set.regimes[0].opponent_contrasts.as_ref().unwrap();
        assert_eq!(contrasts.len(), 2);
        let config = set.regime_config(&set.regimes[0]);
        assert_eq!(
            config.candidate_rows,
            vec![vec![0.0, -1.0, 1.0], vec![-1.0, 0.5, 0.5]]
        );
        assert_eq!(
            contrasts[0].weights,
            vec![0.0, -1.0, 1.0],
            "regime owns the contrasts; the set no longer carries them"
        );
    }

    #[test]
    fn human_fixture_luminance_weights_are_l_m_dominant() {
        let set = human();
        // Pool-wide luminance weighting (§4.2.3): explicit curve-level
        // overrides, all three in the one regime.
        let weights = set.luminance_weights(1.0);
        assert_eq!(weights, vec![0.0, 0.4, 0.6]);
        let config = set.regime_config(&set.regimes[0]);
        assert_eq!(config.luminance_weights, vec![0.0, 0.4, 0.6]);
    }

    #[test]
    fn default_solar_illuminant_has_radiance_quantity_and_peaks_in_visible() {
        let sun = default_solar_illuminant();
        assert!(matches!(sun.quantity, QuantityKind::Radiance { .. }));
        assert_eq!(sun.curve_type, CurveType::Illumination);
        let (_, peak_wl) = sun
            .points
            .iter()
            .map(|&(wl, v)| (v, wl))
            .fold((f64::MIN, 0.0), |a, b| if b.0 > a.0 { b } else { a });
        assert!((400.0..=650.0).contains(&peak_wl), "peak at {peak_wl}nm");
    }

    #[test]
    fn dog_is_n2_dichromat() {
        let set = dog();
        assert_eq!(set.curves.len(), 2);
        assert_eq!(set.regimes[0].name, "Dichromat");
        assert_pipeline_chroma_count(set, 1);
    }

    #[test]
    fn cat_is_n3_with_500nm_mechanism_present_in_chroma() {
        let set = cat();
        assert_eq!(set.curves.len(), 3);
        assert!(
            set.curves.iter().any(|c| c.name.contains("500")),
            "500nm mechanism must not be silently dropped"
        );
        assert_pipeline_chroma_count(set, 2);
    }

    #[test]
    fn honeybee_is_n3_with_literature_contrasts_and_noise_overriding_fallback() {
        let set = honeybee();
        assert_eq!(set.curves.len(), 3);
        assert_eq!(
            set.regimes[0].opponent_contrasts.as_ref().unwrap().len(),
            2,
            "literature contrasts must override the generic fallback"
        );
        assert!(!set.has_partial_eta_coverage());
        assert!(
            set.receptor_noise().is_some(),
            "ΔS should be computable with honeybee's noise data"
        );
        assert_pipeline_chroma_count(set, 2);
    }

    #[test]
    fn pigeon_is_n4_with_effective_curves_narrower_than_raw_opsins() {
        let set = pigeon();
        assert_eq!(
            set.curves.len(),
            12,
            "4 effective + 8 raw opsin/droplet curves in the pool"
        );
        assert_eq!(
            set.regimes[0].curves,
            vec![0, 1, 2, 3],
            "only the 4 effective curves are regime members"
        );
        // The 8 non-member curves belong to no regime (unaffiliated).
        assert!(
            (0..set.curves.len() as u64)
                .filter(|i| set.regimes[0].curves.contains(i))
                .count()
                == 4
        );
        assert_pipeline_chroma_count(set.clone(), 3);

        // Spot-check: the long-wave effective curve's value at a
        // short-wavelength point must be suppressed relative to the raw
        // opsin, since its orange/red droplet should have cut it off.
        let raw_lws = set
            .curves
            .iter()
            .find(|c| c.name == "Long-wave (LWS)")
            .unwrap();
        let effective_lws = set
            .curves
            .iter()
            .find(|c| {
                c.name.starts_with("Long-wave (LWS)")
                    && !c.name.is_empty()
                    && c.name != "Long-wave (LWS)"
            })
            .unwrap();
        let probe_wl = 450.0;
        let raw_v = raw_lws.value_at(probe_wl).unwrap();
        let effective_v = effective_lws.value_at(probe_wl).unwrap();
        assert!(
            effective_v < raw_v * 0.5,
            "raw={raw_v} effective={effective_v}: droplet should suppress short wavelengths"
        );
    }

    #[test]
    fn goldfish_is_n4_using_a2_template_distinct_from_a1() {
        let set = goldfish();
        assert_eq!(set.curves.len(), 4);
        assert_pipeline_chroma_count(set.clone(), 3);

        // A2-generated curve must differ from what A1 would have produced
        // at the same lambda_max (spot-checked away from the shared peak,
        // same reasoning as govardovskii::tests::a2_template_differs...).
        let m_cone = set
            .curves
            .iter()
            .find(|c| c.name.contains("M/green"))
            .unwrap();
        let a1_equivalent = govardovskii::generate_points(530.0, WL_MIN, WL_MAX, STEP);
        let a1_at_probe = a1_equivalent
            .iter()
            .find(|&&(wl, _)| wl == 450.0)
            .unwrap()
            .1;
        let a2_at_probe = m_cone.value_at(450.0).unwrap();
        assert!(
            (a1_at_probe - a2_at_probe).abs() > 0.02,
            "A2 should visibly differ from A1 here"
        );
    }

    #[test]
    fn penguin_is_n3_reduced_tetrachromat() {
        let set = penguin();
        assert_eq!(set.curves.len(), 3);
        assert_pipeline_chroma_count(set, 2);
    }

    #[test]
    fn mouse_is_n2_dichromat_structurally_like_dog() {
        let dog_set = dog();
        let mouse_set = mouse();
        assert_eq!(mouse_set.curves.len(), dog_set.curves.len());
        assert_pipeline_chroma_count(mouse_set, 1);
    }

    #[test]
    fn butterfly_pipeline_sees_exactly_4_not_6_receptors() {
        let set = butterfly();
        assert_eq!(set.curves.len(), 6, "4 color-vision + 2 unaffiliated");
        assert_eq!(
            set.regimes[0].curves,
            vec![0, 1, 2, 3],
            "only the 4 color-vision receptors are regime members"
        );
        assert!(
            set.curves[4].name == "Violet (non-color-vision)"
                && set.curves[5].name == "Broad-band (non-color-vision)"
        );
        assert!(
            !set.regimes
                .iter()
                .flat_map(|r| r.curves.iter())
                .any(|&i| i >= 4),
            "violet + broad-band belong to no regime"
        );
        assert_pipeline_chroma_count(set, 3);
    }

    #[test]
    fn frog_has_three_regimes_with_distinct_membership() {
        let set = frog();
        assert_eq!(set.curves.len(), 5, "2 rods + 3 cones");
        assert_eq!(set.regimes.len(), 3);
        assert_eq!(set.regimes[0].name, "Scotopic");
        assert_eq!(set.regimes[0].curves, vec![0, 1]);
        assert_eq!(
            set.regimes[1].name, "Mesopic",
            "the mesopic regime is its own colorspace, not a scotopic/photopic blend"
        );
        assert_eq!(set.regimes[1].curves, vec![0, 1, 2, 3, 4]);
        assert_eq!(set.regimes[2].name, "Photopic");
        assert_eq!(set.regimes[2].curves, vec![2, 3, 4]);
        // Dimensionality follows membership: 2 receptors -> 1 chroma
        // axis, 5 -> 4, 3 -> 2.
        assert_regime_chroma_count(&set, 0, 1);
        assert_regime_chroma_count(&set, 1, 4);
        assert_regime_chroma_count(&set, 2, 2);
    }

    /// One pipeline per regime, all three same illuminant.
    fn frog_pipelines(power: f64) -> [crate::pipeline::Pipeline; 3] {
        let set = frog();
        let illuminant = SpectralCurve::new("flat", CurveType::Illumination)
            .with_points(vec![(WL_MIN, power), (WL_MAX, power)]);
        [
            crate::pipeline::Pipeline::build(set.regime_config(&set.regimes[0]), &illuminant, 1.0),
            crate::pipeline::Pipeline::build(set.regime_config(&set.regimes[1]), &illuminant, 1.0),
            crate::pipeline::Pipeline::build(set.regime_config(&set.regimes[2]), &illuminant, 1.0),
        ]
    }

    #[test]
    fn frog_regimes_gate_the_other_way_at_opposite_light_levels() {
        // §2.2's whole point for this species: under dim light only the
        // scotopic regime operates, under bright light only the photopic
        // one does, and the mesopic regime - which needs *every* member's
        // window to contain its seen irradiance - collapses at both ends
        // but peaks (and dominates) in the overlap band. Each receptor
        // gates on the irradiance it actually sees from the reference
        // light (E_i = ∫S_i·I dλ, W/m² under the W/m²/nm convention),
        // via the per-curve windows and the real Pipeline. The flat
        // illuminant at amplitude `power` gives each rod/cone a seen
        // irradiance ≈ (template area ~tens of nm)·power, so the chosen
        // powers land decisively inside/outside the windows regardless of
        // the exact template area.
        let [scotopic, mesopic, photopic] = frog_pipelines(1e-7);
        assert!(
            scotopic.is_active(),
            "scotopic should be active in dim light (seen W/m²: {:?})",
            scotopic.seen_irradiances()
        );
        assert!(
            !photopic.is_active(),
            "photopic should be gated off in dim light (seen W/m²: {:?})",
            photopic.seen_irradiances()
        );
        // The mesopic regime in dim light: the rods are operating, but
        // the cones are gated off, so the product over all five members
        // is ~0 while `is_active` (any member on) is still true - the
        // blurry-threshold notion a single pipeline can never expose.
        assert!(
            mesopic.is_active(),
            "individual rods are on even in dim light"
        );
        assert!(
            mesopic.regime_gain() < 1e-3,
            "but the whole-regime product collapses: {}",
            mesopic.regime_gain()
        );
        assert!(
            scotopic.regime_gain() > mesopic.regime_gain()
                && scotopic.regime_gain() > photopic.regime_gain(),
            "scotopic dominates in dim light: s={} m={} p={}",
            scotopic.regime_gain(),
            mesopic.regime_gain(),
            photopic.regime_gain()
        );

        let [scotopic, mesopic, photopic] = frog_pipelines(1.0);
        assert!(
            !scotopic.is_active(),
            "scotopic should be gated off in bright light (seen W/m²: {:?})",
            scotopic.seen_irradiances()
        );
        assert!(
            photopic.is_active(),
            "photopic should be active in bright light (seen W/m²: {:?})",
            photopic.seen_irradiances()
        );
        assert!(
            mesopic.regime_gain() < 1e-3,
            "mesopic collapses in bright light too (cones on, rods off): {}",
            mesopic.regime_gain()
        );
        assert!(
            photopic.regime_gain() > mesopic.regime_gain()
                && photopic.regime_gain() > scotopic.regime_gain(),
            "photopic dominates in bright light: p={} m={} s={}",
            photopic.regime_gain(),
            mesopic.regime_gain(),
            scotopic.regime_gain()
        );

        // Mid-band: pick the illuminant power from the photopic
        // pipeline's own geometry so the cones sit mid-window (2e-2
        // W/m² seen, in [1e-2, ∞)), scaling the *seen* levels rather
        // than guessing template areas. Rods then see ~the same 2e-2
        // (their templates are the same order of area), which is inside
        // [1e-6, 3e-2] but below 3e-2's shoulder - both classes on.
        let set = frog();
        let ref_illuminant = SpectralCurve::new("flat", CurveType::Illumination)
            .with_points(vec![(WL_MIN, 1.0), (WL_MAX, 1.0)]);
        let photopic_pipe = crate::pipeline::Pipeline::build(
            set.regime_config(&set.regimes[2]),
            &ref_illuminant,
            1.0,
        );
        let cone_area_at_unit = photopic_pipe.seen_irradiances()[0];
        let mid_power = 2e-2 / cone_area_at_unit;

        let [scotopic, mesopic, photopic] = frog_pipelines(mid_power);
        assert!(
            scotopic.is_active() && photopic.is_active(),
            "in the mesopic band both receptor classes operate (s: {:?}, p: {:?})",
            scotopic.response_gains(),
            photopic.response_gains()
        );
        // The mesopic regime needs *every one of its five members'*
        // windows to contain its seen irradiance, so its product is
        // necessarily below either single-class product in the band -
        // what it does better than anything is peak *here*. It collapses
        // at both ends (a member class is gated off) and is substantively
        // active only mid-band.
        let dim_mesopic = frog_pipelines(1e-7)[1].regime_gain();
        let bright_mesopic = frog_pipelines(1.0)[1].regime_gain();
        assert!(
            mesopic.regime_gain() > 0.05
                && mesopic.regime_gain() > dim_mesopic
                && mesopic.regime_gain() > bright_mesopic,
            "the mesopic regime is most active in its band: m={} (dim {dim_mesopic}, bright {bright_mesopic})",
            mesopic.regime_gain()
        );
    }

    #[test]
    fn frog_photopic_fork_removing_contested_cone_yields_independent_n2_set() {
        // The documented Save-As workflow: fork the whole frog, drop the
        // contested middle cone from the pool and every regime that
        // listed it, confirm the photopic subset is an independent N=2
        // set that doesn't touch the original.
        let original = frog();
        let mut forked = original.clone();
        forked.name = "Frog (Rana spp.) — 2-cone photopic variant".to_string();
        let idx = forked
            .curves
            .iter()
            .position(|c| c.name.contains("Middle"))
            .unwrap();
        forked.curves.remove(idx);
        for r in forked.regimes.iter_mut() {
            r.curves.retain(|&i| i as usize != idx);
            for i in r.curves.iter_mut() {
                if *i as usize > idx {
                    *i -= 1;
                }
            }
        }

        let photopic_idx = forked
            .regimes
            .iter()
            .position(|r| r.name == "Photopic")
            .unwrap();
        assert_eq!(
            forked.regimes[photopic_idx].curves.len(),
            2,
            "photopic now a 2-cone colorspace"
        );
        assert_eq!(
            original.regimes[2].curves.len(),
            3,
            "the original must be untouched"
        );
        // Pool still holds the 2 remaining rods too; the photopic regime
        // correctly skips them (indices 2,3 -> blue + red cones).
        assert_eq!(forked.regimes[photopic_idx].curves, vec![2, 3]);
        assert_regime_chroma_count(&forked, photopic_idx as usize, 1);
    }

    #[test]
    fn mantis_shrimp_has_no_colorspace_regimes_at_all() {
        let set = mantis_shrimp();
        assert_eq!(set.curves.len(), 12);
        assert!(set.regimes.is_empty(), "zero colorspace regimes by design");

        // The app builds no pipeline for a regime-less species; the
        // degenerate default config still builds without panicking (the
        // N=0 path), and reports zero luminance/chroma as this fixture's
        // whole premise requires.
        let illuminant = SpectralCurve::new("flat", CurveType::Illumination)
            .with_points(vec![(WL_MIN, 1.0), (WL_MAX, 1.0)]);
        let pipeline =
            crate::pipeline::Pipeline::build(set.default_regime_config(), &illuminant, 1.0);
        let stimulus = SpectralCurve::new("test", CurveType::Reflectance)
            .with_points(vec![(400.0, 0.5), (700.0, 0.5)]);
        let coords = pipeline.coordinates(&stimulus);
        assert_eq!(coords.luminance, 0.0);
        assert_eq!(coords.chroma, Vec::<f64>::new());
        assert_eq!(
            pipeline.noise(),
            None,
            "ΔS must be unavailable, not fabricated"
        );
        assert!(!pipeline.is_active());

        // The actually-meaningful output: every receptor reported
        // individually (pool-wide gains + raw activations) is finite.
        let seen = crate::pipeline::seen_irradiances_for(&set.curves, &illuminant, 1.0);
        let gains = set.receptor_gains(&seen);
        assert_eq!(gains.len(), 12);
        assert!(gains.iter().all(|g| *g == 1.0), "no windows anywhere");
        let activations =
            crate::pipeline::raw_activations_with_gains(&set.curves, &gains, &stimulus, 1.0);
        assert_eq!(activations.len(), 12);
        assert!(activations.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn all_11_builtin_fixtures_load_and_have_distinct_names() {
        let fixtures = builtin_fixtures();
        assert_eq!(fixtures.len(), 11, "frog's two files merged into one");
        let mut names: Vec<&str> = fixtures.iter().map(|f| f.name.as_str()).collect();
        names.sort();
        names.dedup();
        assert_eq!(
            names.len(),
            fixtures.len(),
            "every fixture must have a distinct name"
        );
        for set in &fixtures {
            assert!(!set.curves.is_empty(), "{} has no receptors", set.name);
            // The exact set can be a regime-less species (mantis) or a
            // single/multi-regime one - never both empty membership and
            // empty regimes simultaneously with nothing to show.
            assert!(
                !set.regimes.iter().all(|r| r.curves.is_empty()) || set.regimes.is_empty(),
                "{} keeps only empty regimes around",
                set.name
            );
        }
    }

    #[test]
    fn representability_audit_no_fixture_needs_species_specific_pipeline_code() {
        // The practical verification of §8.2's representability claim:
        // every fixture's *every* regime must build a working Pipeline
        // through the exact same generic path, with no special-casing by
        // name anywhere in this test (if one fixture needed different
        // handling, this loop itself would have to branch on species -
        // it doesn't). Regime-less species (mantis) take the degenerate
        // default-config path with zero chroma.
        for set in builtin_fixtures() {
            let illuminant = SpectralCurve::new("flat", CurveType::Illumination)
                .with_points(vec![(WL_MIN, 1.0), (WL_MAX, 1.0)]);
            if set.regimes.is_empty() {
                let pipeline =
                    crate::pipeline::Pipeline::build(set.default_regime_config(), &illuminant, 1.0);
                assert_eq!(pipeline.chroma_axis_count(), 0, "{}", set.name);
                continue;
            }
            for (i, regime) in set.regimes.iter().enumerate() {
                let n = set.regime_config(regime).curves.len();
                let pipeline =
                    crate::pipeline::Pipeline::build(set.regime_config(regime), &illuminant, 1.0);
                assert_eq!(
                    pipeline.chroma_axis_count(),
                    n.saturating_sub(1),
                    "{} regime {i} ({}): expected N-1 chroma channels",
                    set.name,
                    regime.name
                );
            }
        }
    }
}
