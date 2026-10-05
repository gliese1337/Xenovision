//! Species fixture builders (design doc §2.3, §8). File-based packaging
//! (loading/saving these as separate user-editable files, §8.3) lives in
//! `fixture_library`, built on top of the plain in-memory `CurveSet`
//! values this module returns.

use crate::blackbody;
use crate::curve::{CurveType, QuantityKind, SpectralCurve};
use crate::curve_set::{CurveSet, OpponentContrast};
use crate::govardovskii::{self, Chromophore};
use crate::oil_droplet;

const WL_MIN: f64 = 300.0;
const WL_MAX: f64 = 750.0;
const STEP: f64 = 5.0;

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
/// 420/530/560nm. Curve order is [S, M, L].
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

    set.colorspace_curves = vec![s_cone, m_cone, l_cone];

    // Literature-standard red-green and blue-yellow contrasts (§2.3.1),
    // exercised through real CurveSet data per §8.2's representability
    // requirement.
    set.opponent_contrasts = vec![
        OpponentContrast {
            name: "L - M (red-green)".to_string(),
            weights: vec![0.0, -1.0, 1.0],
        },
        OpponentContrast {
            name: "S - (M+L)/2 (blue-yellow)".to_string(),
            weights: vec![1.0, -0.5, -0.5],
        },
    ];

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
    set.colorspace_curves = vec![
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
    set.colorspace_curves = vec![
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

    set.colorspace_curves = vec![uv, blue, green];

    set.opponent_contrasts = vec![
        OpponentContrast {
            name: "UV - Blue".to_string(),
            weights: vec![1.0, -1.0, 0.0],
        },
        OpponentContrast {
            name: "Green - (UV+Blue)/2".to_string(),
            weights: vec![-0.5, -0.5, 1.0],
        },
    ];

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
/// opsin and droplet curves are kept isolated for inspection; only the
/// 4 composited effective curves are in the colorspace, pipeline-facing
/// set.
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
    let mut isolated_curves = Vec::new();
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
        isolated_curves.push(opsin);
        isolated_curves.push(droplet);
    }

    set.colorspace_curves = effective_curves;
    set.isolated_curves = isolated_curves;

    set.metadata.insert(
        "note".to_string(),
        "Red-field droplet values used as the representative default (pigeons have two retinal \
         fields - red and yellow - with different droplet populations, not modeled separately)."
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
    set.colorspace_curves = vec![
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
    set.colorspace_curves = vec![
        cone_curve("Violet-sensitive (SWS1)", 403.0, "Bowmaker & Martin 1985"),
        cone_curve("S/blue receptor", 450.0, "Bowmaker & Martin 1985"),
        cone_curve("L/green receptor", 543.0, "Bowmaker & Martin 1985"),
    ];
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
    set.colorspace_curves = vec![
        cone_curve("S-cone (UV)", 360.0, "Jacobs, Neitz & Deegan 1991"),
        cone_curve("M-cone (green)", 508.0, "Jacobs, Neitz & Deegan 1991"),
    ];
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
/// color vision (UV/blue/green/red at 360/460/520/600nm - the
/// colorspace, pipeline-facing set), plus two further measured receptor
/// classes that participate in non-chromatic vision (motion processing)
/// rather than color opponency, stored as isolated curves instead of
/// the colorspace set per §2.2.4's exception-free "every receptor in
/// the colorspace set contributes to both luminance and chroma" rule -
/// demonstrating `CurveSet::isolated_curves` rather than requiring an
/// exception to that rule.
pub fn butterfly() -> CurveSet {
    let mut set = CurveSet::new("Swallowtail Butterfly (Papilio xuthus)");
    set.colorspace_curves = vec![
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
         itself isn't modeled, since this curve is isolated and not used by the pipeline."
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

    set.isolated_curves = vec![violet, broad_band];

    set.metadata.insert(
        "note".to_string(),
        "The 4 color-vision receptors (UV/Blue/Green/Red) are the colorspace Curve Set every \
         calculation operates on. Two further measured receptor classes (violet, broad-band) \
         are documented in the literature as participating in non-chromatic vision rather than \
         color opponency, and are kept isolated for completeness - not part of the N=4 the \
         pipeline sees, per §2.2.4's exception-free all-receptors-contribute rule."
            .to_string(),
    );
    note_generic_fallback(&mut set);
    set
}

/// Frog (*Rana* spp.) scotopic (dim-light, rod-based) system, §2.3.10:
/// green/red rods at 433/502nm. Entirely independent of `frog_photopic`
/// below - no mesopic (rod+cone combination) calculation is modeled,
/// per the design doc's explicit scoping note.
pub fn frog_scotopic() -> CurveSet {
    let mut set = CurveSet::new("Frog (Rana spp.) — Scotopic");
    set.colorspace_curves = vec![
        cone_curve(
            "Green rod",
            433.0,
            "Denton & Wyllie 1955; Yovanovich et al. 2017",
        ),
        cone_curve(
            "Red rod",
            502.0,
            "Denton & Wyllie 1955; Yovanovich et al. 2017",
        ),
    ];
    set.metadata.insert(
        "note".to_string(),
        "The dual rod system is functionally active at the same (very low) light levels - the \
         behavioral precondition for the scotopic colour discrimination documented in the cited \
         studies. Entirely independent of the photopic fixture: separate file, no shared \
         calculation state, per the design doc's explicit no-mesopic-combination scoping."
            .to_string(),
    );
    note_generic_fallback(&mut set);
    set
}

/// Frog (*Rana* spp.) photopic (bright-light, cone-based) system,
/// §2.3.10: blue/middle/red cones at 431/502/580nm - N=3 as the
/// documented default, though the 502nm middle cone's existence is
/// contested in some species/studies (see metadata). Entirely
/// independent of `frog_scotopic` above.
pub fn frog_photopic() -> CurveSet {
    let mut set = CurveSet::new("Frog (Rana spp.) — Photopic");
    set.colorspace_curves = vec![
        cone_curve(
            "Blue cone",
            431.0,
            "Koskelainen, Hemilä & Donner 1994; Liebman & Entine 1968",
        ),
        cone_curve(
            "Middle/green cone",
            502.0,
            "Koskelainen, Hemilä & Donner 1994; Liebman & Entine 1968",
        ),
        cone_curve(
            "Red cone",
            580.0,
            "Koskelainen, Hemilä & Donner 1994; Liebman & Entine 1968",
        ),
    ];
    set.metadata.insert(
        "contested_cone_note".to_string(),
        "The 502nm middle cone's status is contested: confirmed via microspectrophotometry in \
         some species/studies, reported absent/contentious in others (e.g. R. temporaria per the \
         Koskelainen lineage of work). This N=3 default is a documented assumption to verify per \
         species/study, not a settled fact - fork via Save As and remove this cone to model the \
         2-cone (red+blue) alternative."
            .to_string(),
    );
    set.metadata.insert(
        "note".to_string(),
        "Entirely independent of the scotopic fixture: separate file, no shared calculation \
         state, per the design doc's two-independent-systems framing (§2.3.10)."
            .to_string(),
    );
    note_generic_fallback(&mut set);
    set
}

/// Mantis shrimp (Stomatopoda, e.g. *Odontodactylus scyllarus*), §10.2:
/// the motivating real-world example for that TODO - a species with far
/// *more* photoreceptor classes than any other fixture here (12, vs.
/// pigeon/goldfish's 4), yet **none** of them participate in the §2.2.4
/// opponent-process colorspace construction every other species'
/// colorspace set does. Behavioral/electrophysiological work (notably
/// Thoen et al. 2014's "chunk" hypothesis) found mantis shrimp color
/// *discrimination* performance is markedly worse than a human
/// trichromat's despite the much larger receptor count - the leading
/// explanation is that each receptor class is read out largely
/// independently (e.g. via rapid eye-scanning/temporal comparison across
/// the midband rows) rather than combined through opponent-process
/// channels the way vertebrate color vision in every other fixture here
/// works. This fixture models that directly: the entire receptor set
/// lives in `isolated_curves`; the colorspace set is **empty** - the
/// first fixture with zero colorspace receptors, exercising the N=0
/// path through `Pipeline`/`derive_adaptation_matrix` (both handle it
/// generically, same as the existing N=1 monochromat case -
/// no species-specific code needed here either, consistent with §8.2).
/// §10.1's activation display is what makes this fixture's data visible
/// at all: `Pipeline::coordinates` always returns `luminance = 0.0`,
/// `chroma = []` for it by construction (there's nothing in the
/// colorspace set to weight), so the only meaningful output is each
/// isolated curve's individual raw activation.
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
    set.isolated_curves = classes
        .iter()
        .map(|(name, lmax)| {
            cone_curve(
                name,
                *lmax,
                "Cronin & Marshall 1989; Marshall, Land & Cronin 2007; Thoen et al. 2014",
            )
        })
        .collect();
    set.colorspace_curves = Vec::new();

    set.metadata.insert(
        "note".to_string(),
        "12 photoreceptor classes (illustrative λmax values spanning UV to red - the headline \
         '12 types' figure is well-established, but these specific wavelengths weren't \
         re-verified digit-by-digit against primary microspectrophotometry data, same confidence \
         caveat as every other fixture's approximated values). All 12 live in isolated_curves; \
         the colorspace set is intentionally empty - no species in this app has more raw \
         photoreceptor diversity, and none has less opponent-process participation. Luminance \
         and chroma are always exactly 0 for this fixture by construction (nothing in the \
         colorspace set to weight) - the direct consequence of modeling the hypothesis that \
         these channels aren't combined into a human-style opponent colorspace at all. The \
         individual receptor activations (via Pipeline::colorspace_activations/isolated_activations, \
         §10.1) are this fixture's only meaningful output."
            .to_string(),
    );
    set.metadata.insert(
        "opponent_pairing_note".to_string(),
        "Not applicable: zero colorspace receptors means zero opponent contrasts by \
         construction, not an unmodeled literature mechanism."
            .to_string(),
    );
    set.metadata.insert(
        "noise_data_note".to_string(),
        "Not applicable: ΔS needs activity in the colorspace set, which is empty here by design."
            .to_string(),
    );
    set
}

/// All 12 fixtures this app ships with (human + the 9 species/systems
/// above + frog's 2nd regime + mantis shrimp) - the embedded pristine
/// defaults `fixture_library`'s "restore defaults" regenerates the
/// user-visible files from.
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
        frog_scotopic(),
        frog_photopic(),
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

    #[test]
    fn human_fixture_has_three_cones_in_s_m_l_order() {
        let set = human();
        assert_eq!(set.colorspace_curves.len(), 3);
        assert_eq!(set.colorspace_curves[0].name, "S-cone");
        assert_eq!(set.colorspace_curves[1].name, "M-cone");
        assert_eq!(set.colorspace_curves[2].name, "L-cone");
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
        assert_eq!(set.opponent_contrasts.len(), 2);
        assert_eq!(
            set.candidate_rows(),
            vec![vec![0.0, -1.0, 1.0], vec![1.0, -0.5, -0.5]]
        );
    }

    #[test]
    fn human_fixture_luminance_weights_are_l_m_dominant() {
        let set = human();
        let weights = set.luminance_weights(1.0);
        assert_eq!(weights, vec![0.0, 0.4, 0.6]);
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

    /// Builds a Pipeline for `set` against a flat illuminant and checks
    /// it produces exactly `expected_chroma` chroma channels - the
    /// generic shape check every species fixture below needs (N
    /// receptors -> N-1 chroma channels), run through the pipeline
    /// rather than just inspecting the CurveSet's shape, so it also
    /// catches a fixture that fails to actually build (e.g. a curve
    /// with no points).
    fn assert_pipeline_chroma_count(set: CurveSet, expected_chroma: usize) {
        let illuminant = SpectralCurve::new("flat", CurveType::Illumination)
            .with_points(vec![(WL_MIN, 1.0), (WL_MAX, 1.0)]);
        let pipeline = crate::pipeline::Pipeline::build(set, &illuminant, 1.0);
        assert_eq!(pipeline.chroma_axis_count(), expected_chroma);
    }

    #[test]
    fn dog_is_n2_dichromat() {
        let set = dog();
        assert_eq!(set.colorspace_curves.len(), 2);
        assert_pipeline_chroma_count(set, 1);
    }

    #[test]
    fn cat_is_n3_with_500nm_mechanism_present_in_chroma() {
        let set = cat();
        assert_eq!(set.colorspace_curves.len(), 3);
        assert!(
            set.colorspace_curves.iter().any(|c| c.name.contains("500")),
            "500nm mechanism must not be silently dropped"
        );
        assert_pipeline_chroma_count(set, 2);
    }

    #[test]
    fn honeybee_is_n3_with_literature_contrasts_and_noise_overriding_fallback() {
        let set = honeybee();
        assert_eq!(set.colorspace_curves.len(), 3);
        assert_eq!(
            set.opponent_contrasts.len(),
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
            set.colorspace_curves.len(),
            4,
            "4 effective (opsin x droplet) curves in the colorspace set"
        );
        assert_eq!(
            set.isolated_curves.len(),
            8,
            "4 raw opsins + 4 droplets kept isolated"
        );
        assert_pipeline_chroma_count(set.clone(), 3);

        // Spot-check: the long-wave effective curve's value at a
        // short-wavelength point must be suppressed relative to the raw
        // opsin, since its orange/red droplet should have cut it off.
        let raw_lws = set
            .isolated_curves
            .iter()
            .find(|c| c.name == "Long-wave (LWS)")
            .unwrap();
        let effective_lws = set
            .colorspace_curves
            .iter()
            .find(|c| c.name.starts_with("Long-wave (LWS)"))
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
        assert_eq!(set.colorspace_curves.len(), 4);
        assert_pipeline_chroma_count(set.clone(), 3);

        // A2-generated curve must differ from what A1 would have produced
        // at the same lambda_max (spot-checked away from the shared peak,
        // same reasoning as govardovskii::tests::a2_template_differs...).
        let m_cone = set
            .colorspace_curves
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
        assert_eq!(set.colorspace_curves.len(), 3);
        assert_pipeline_chroma_count(set, 2);
    }

    #[test]
    fn mouse_is_n2_dichromat_structurally_like_dog() {
        let dog_set = dog();
        let mouse_set = mouse();
        assert_eq!(
            mouse_set.colorspace_curves.len(),
            dog_set.colorspace_curves.len()
        );
        assert_pipeline_chroma_count(mouse_set, 1);
    }

    #[test]
    fn butterfly_pipeline_sees_exactly_4_not_6_receptors() {
        let set = butterfly();
        assert_eq!(
            set.colorspace_curves.len(),
            4,
            "only the 4 color-vision receptors are in the colorspace set"
        );
        assert_eq!(
            set.isolated_curves.len(),
            2,
            "violet + broad-band are isolated"
        );
        assert_pipeline_chroma_count(set, 3);
    }

    #[test]
    fn frog_scotopic_and_photopic_are_independent_n2_and_n3() {
        let scotopic = frog_scotopic();
        let photopic = frog_photopic();
        assert_eq!(scotopic.colorspace_curves.len(), 2);
        assert_eq!(photopic.colorspace_curves.len(), 3);
        assert_ne!(scotopic.name, photopic.name);
        // Independence is structural (two separate CurveSet values, no
        // shared mutable state) - demonstrated by just being able to
        // build and use both without one affecting the other.
        assert_pipeline_chroma_count(scotopic, 1);
        assert_pipeline_chroma_count(photopic, 2);
    }

    #[test]
    fn frog_photopic_fork_removing_contested_cone_yields_independent_n2_set() {
        // The documented Save-As workflow: fork photopic, drop the
        // contested middle cone, confirm the result is an independent
        // N=2 set that doesn't touch the original.
        let original = frog_photopic();
        let mut forked = original.clone();
        forked.name = "Frog (Rana spp.) — Photopic, 2-cone variant".to_string();
        let idx = forked
            .colorspace_curves
            .iter()
            .position(|c| c.name.contains("Middle"))
            .unwrap();
        forked.colorspace_curves.remove(idx);

        assert_eq!(forked.colorspace_curves.len(), 2);
        assert_eq!(
            original.colorspace_curves.len(),
            3,
            "the original must be untouched"
        );
        assert_pipeline_chroma_count(forked, 1);
    }

    #[test]
    fn mantis_shrimp_is_n0_all_isolated_curves() {
        let set = mantis_shrimp();
        assert_eq!(
            set.colorspace_curves.len(),
            0,
            "colorspace set must be empty by design"
        );
        assert_eq!(set.isolated_curves.len(), 12);

        // Builds a Pipeline with zero colorspace curves without
        // panicking (the N=0 path through derive_adaptation_matrix),
        // and produces the degenerate-but-correct zero luminance/chroma
        // this fixture's whole premise depends on.
        assert_pipeline_chroma_count(set.clone(), 0);
        let illuminant = SpectralCurve::new("flat", CurveType::Illumination)
            .with_points(vec![(WL_MIN, 1.0), (WL_MAX, 1.0)]);
        let pipeline = crate::pipeline::Pipeline::build(set, &illuminant, 1.0);
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

        // The actually-meaningful output: individual receptor
        // activations are still computable and finite for all 12
        // isolated curves.
        let activations = pipeline.isolated_activations(&stimulus);
        assert_eq!(activations.len(), 12);
        assert!(activations.iter().all(|v| v.is_finite()));
        assert_eq!(pipeline.isolated_curve_names().len(), 12);
    }

    #[test]
    fn all_12_builtin_fixtures_load_and_have_distinct_names() {
        let fixtures = builtin_fixtures();
        assert_eq!(fixtures.len(), 12);
        let mut names: Vec<&str> = fixtures.iter().map(|f| f.name.as_str()).collect();
        names.sort();
        names.dedup();
        assert_eq!(
            names.len(),
            fixtures.len(),
            "every fixture must have a distinct name"
        );
        for set in &fixtures {
            // Mantis shrimp (§10.2) is the one deliberate exception: zero
            // colorspace curves, all 12 receptors isolated instead.
            assert!(
                !set.colorspace_curves.is_empty() || !set.isolated_curves.is_empty(),
                "{} has no curves anywhere",
                set.name
            );
        }
    }

    #[test]
    fn representability_audit_no_fixture_needs_species_specific_pipeline_code() {
        // The practical verification of §8.2's representability claim:
        // every fixture must build a working Pipeline through the exact
        // same generic path, with no special-casing by name anywhere in
        // this test (if one fixture needed different handling, this loop
        // itself would have to branch on species - it doesn't).
        for set in builtin_fixtures() {
            let illuminant = SpectralCurve::new("flat", CurveType::Illumination)
                .with_points(vec![(WL_MIN, 1.0), (WL_MAX, 1.0)]);
            let name = set.name.clone();
            let n = set.colorspace_curves.len();
            let pipeline = crate::pipeline::Pipeline::build(set, &illuminant, 1.0);
            assert_eq!(
                pipeline.chroma_axis_count(),
                n.saturating_sub(1),
                "{name}: expected N-1 chroma channels"
            );
        }
    }
}
