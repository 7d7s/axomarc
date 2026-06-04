(function () {
  var routes = {
    "social-media": "social-media.html",
    "brand-identity": "brand-identity.html",
    "content-creation": "content-creation.html",
    "paid-campaigns": "paid-campaigns.html",
    "website-experience": "website-experience.html",
    "creative-strategy": "creative-strategy.html"
  };

  var services = {
    "social-media": {
      title: "Social Media",
      headline: "Build Attention. Create Presence.",
      icon: "fa-share-alt",
      intro: "We create social media systems that help your brand show up with clarity, consistency, and content people actually want to engage with.",
      about: "Social media is where people first feel your brand's energy. AXOMARC builds clear content systems, platform direction, and creative rhythm so your online presence feels consistent, recognizable, and worth following.",
      metricOne: "+64%",
      metricOneLabel: "Engagement",
      metricTwo: "3.8x",
      metricTwoLabel: "Reach Lift",
      offers: [
        ["Social Media Strategy", "fa-compass", "Audience, positioning, platform priorities, and content direction shaped before posting starts."],
        ["Content Planning & Calendars", "fa-calendar-alt", "Organized monthly content plans designed around themes, timing, and campaign flow."],
        ["Instagram Management", "fa-instagram", "Profile direction, posting systems, captions, visual rhythm, and engagement support."],
        ["Campaign Concepts", "fa-lightbulb", "Creative ideas built to make launches, offers, and stories easier to notice."],
        ["Performance Review", "fa-chart-line", "Clear reviews of reach, engagement, saves, shares, and audience response."],
        ["Community Direction", "fa-comments", "Guidance for conversations, comments, and brand tone across active channels."]
      ],
      why: "Social media is often the first place people experience your brand. A strong presence builds recognition, trust, and daily connection before a customer ever sends an inquiry.",
      proofs: ["Stronger visibility", "Better engagement", "Audience connection", "Trust building"],
      process: [
        "We study your audience, content gaps, competitors, and current presence.",
        "We define content pillars, posting rhythm, and creative direction.",
        "We build visuals, captions, reel ideas, and campaign-ready assets.",
        "We review performance and refine what your audience responds to."
      ],
      related: "Content systems designed to create attention and engagement."
    },
    "brand-identity": {
      title: "Brand Identity",
      headline: "Build A Brand That Feels Instantly Recognizable.",
      icon: "fa-layer-group",
      intro: "We shape visual identity systems that make your brand recognizable, premium, and consistent across every digital touchpoint.",
      about: "A brand identity gives your business a visual memory. AXOMARC develops the logo direction, color behavior, type style, and visual rules that help your brand look distinct before a customer reads a single line.",
      metricOne: "Clear",
      metricOneLabel: "Brand Recall",
      metricTwo: "360",
      metricTwoLabel: "Visual Direction",
      offers: [
        ["Logo Direction", "fa-pen-nib", "Distinct logo concepts and usage direction built around recognition and clarity."],
        ["Color & Typography", "fa-palette", "Premium color systems and type choices that support your tone and audience."],
        ["Brand Guidelines", "fa-book-open", "Simple rules for keeping your identity consistent across every channel."],
        ["Visual Language", "fa-shapes", "Patterns, layout behavior, graphic accents, and design details that feel ownable."],
        ["Social Identity Kits", "fa-th-large", "Profile, post, story, and campaign assets aligned with the brand system."],
        ["Launch Assets", "fa-rocket", "Core creative materials to help introduce or refresh your brand with confidence."]
      ],
      why: "A strong identity makes your business easier to recognize and easier to trust. It gives every visual decision a reason and helps your brand feel consistent everywhere.",
      proofs: ["Clear recognition", "Consistent communication", "Premium positioning", "Long-term trust"],
      process: [
        "We understand your market, audience, tone, and visual references.",
        "We define positioning, personality, and identity direction.",
        "We design the visual system and supporting brand assets.",
        "We prepare identity rules that can scale across campaigns."
      ],
      related: "Visual identity built to feel distinct instantly."
    },
    "content-creation": {
      title: "Content Creation",
      headline: "Make Visuals People Stop For.",
      icon: "fa-photo-video",
      intro: "We create scroll-stopping photo, graphic, and video content designed to communicate your brand with clarity and emotion.",
      about: "Content turns strategy into something people can see and remember. AXOMARC creates platform-ready visuals, videos, graphics, and copy direction that make your brand feel active, clear, and emotionally connected.",
      metricOne: "High",
      metricOneLabel: "Content Recall",
      metricTwo: "Fast",
      metricTwoLabel: "Creative Cycles",
      offers: [
        ["Campaign Visuals", "fa-images", "Premium graphics and visuals created around campaign stories and audience attention."],
        ["Reels & Short Videos", "fa-video", "Short-form video ideas, edits, and content direction built for modern platforms."],
        ["Graphic Design", "fa-bezier-curve", "Social posts, ads, announcements, carousels, and branded creative systems."],
        ["Product Content", "fa-cube", "Visual content that presents your product or service with clarity and appeal."],
        ["Copy Direction", "fa-pen", "Caption, hook, and message direction that supports the visual story."],
        ["Content Adaptations", "fa-clone", "Assets resized and adapted for different placements, channels, and campaigns."]
      ],
      why: "Content turns strategy into something people can see, feel, and remember. Strong content helps your brand stay active, relevant, and emotionally connected.",
      proofs: ["Scroll-stopping visuals", "Platform-ready assets", "Stronger storytelling", "Better brand memory"],
      process: [
        "We identify the content formats and messages your audience needs.",
        "We plan creative themes, shot direction, and asset priorities.",
        "We produce visuals, videos, graphics, and campaign content.",
        "We refine content based on attention, saves, shares, and response."
      ],
      related: "Scroll-stopping visuals crafted for modern audiences."
    },
    "paid-campaigns": {
      title: "Paid Campaigns",
      headline: "Turn Creative Into Measurable Growth.",
      icon: "fa-bullseye",
      intro: "We build paid advertising campaigns that combine sharp targeting, strong creative, and performance-focused execution.",
      about: "Paid campaigns perform best when the offer, audience, message, and creative move together. AXOMARC builds campaign systems that look premium, communicate quickly, and give your business clearer growth signals.",
      metricOne: "+42%",
      metricOneLabel: "Lead Quality",
      metricTwo: "ROAS",
      metricTwoLabel: "Focused",
      offers: [
        ["Campaign Strategy", "fa-route", "Campaign structure, objective selection, message direction, and conversion planning."],
        ["Ad Creative Direction", "fa-magic", "Visual and copy direction designed to catch attention without losing brand quality."],
        ["Audience Targeting", "fa-users", "Audience groups and targeting logic shaped around your customer and offer."],
        ["Meta Ads Setup", "fa-bullhorn", "Campaign setup support for Meta placements, ad sets, creatives, and launch checks."],
        ["Landing Flow Review", "fa-window-maximize", "Review of landing pages and inquiry flow so clicks have a stronger next step."],
        ["Reporting & Optimization", "fa-chart-pie", "Performance review, creative learning, and improvement direction after launch."]
      ],
      why: "Paid campaigns work best when strategy and creative move together. The right message, audience, and visual can turn attention into real business outcomes.",
      proofs: ["Sharper targeting", "Better lead quality", "Creative testing", "Performance learning"],
      process: [
        "We review your audience, offer, competitors, and campaign goals.",
        "We map the campaign structure, message, and conversion path.",
        "We develop ad visuals, copy, and platform-ready assets.",
        "We monitor performance and improve based on campaign data."
      ],
      related: "Performance-focused marketing with creative impact."
    },
    "website-experience": {
      title: "Website Experience",
      headline: "Build Digital Spaces That Convert.",
      icon: "fa-desktop",
      intro: "We design website experiences that make your brand feel professional, easy to understand, and ready for action.",
      about: "Your website is the place where interest becomes trust. AXOMARC creates digital experiences with clear hierarchy, clean motion, responsive layouts, and conversion-focused content flow.",
      metricOne: "Clean",
      metricOneLabel: "User Journey",
      metricTwo: "Fast",
      metricTwoLabel: "Decision Flow",
      offers: [
        ["Website Strategy", "fa-sitemap", "Page structure, user journey, content priorities, and conversion sections."],
        ["Landing Pages", "fa-file-alt", "Focused pages built for campaigns, offers, launches, and lead generation."],
        ["UI Direction", "fa-columns", "Clean interface direction with premium hierarchy, spacing, and interaction states."],
        ["Responsive Layouts", "fa-mobile-alt", "Layouts planned for desktop, tablet, and mobile without awkward overflow."],
        ["Conversion Sections", "fa-mouse-pointer", "Trust points, CTA areas, proof blocks, and inquiry flow designed for action."],
        ["Website Content Flow", "fa-align-left", "Organized copy and section sequencing that makes the brand easier to understand."]
      ],
      why: "Your website is where interest becomes action. A clear experience helps visitors understand your value, trust your brand, and take the next step.",
      proofs: ["Clear navigation", "Better trust", "Conversion-focused flow", "Professional presence"],
      process: [
        "We understand users, goals, pages, and the decisions visitors need to make.",
        "We structure the content, sections, and conversion journey.",
        "We design responsive layouts with strong hierarchy and clean motion.",
        "We improve flow around clarity, trust, and action points."
      ],
      related: "Digital spaces designed with motion and storytelling."
    },
    "creative-strategy": {
      title: "Creative Strategy",
      headline: "Give Every Idea A Clear Reason.",
      icon: "fa-lightbulb",
      intro: "We shape the creative direction behind campaigns, content, and brand decisions so every visual supports a bigger goal.",
      about: "Creative strategy gives your brand a reason behind every message and visual. AXOMARC connects audience insight, positioning, content direction, and campaign ideas into one clear creative system.",
      metricOne: "Focused",
      metricOneLabel: "Creative System",
      metricTwo: "Clear",
      metricTwoLabel: "Messaging",
      offers: [
        ["Brand Positioning", "fa-crosshairs", "Clear direction for how your brand should be understood, remembered, and chosen."],
        ["Campaign Ideas", "fa-lightbulb", "Concepts shaped around emotion, attention, and business goals."],
        ["Messaging Direction", "fa-comment-dots", "Core messages, hooks, and story angles that guide creative execution."],
        ["Creative Roadmaps", "fa-map", "Organized creative priorities for content, launches, campaigns, and brand activity."],
        ["Audience Insights", "fa-user-check", "Understanding what your audience needs to notice, trust, and act on."],
        ["Execution Planning", "fa-tasks", "Turning direction into clear next steps for design, content, and marketing."]
      ],
      why: "Creative work becomes stronger when every choice has intention. Strategy connects your goals, audience, message, and execution into one clear direction.",
      proofs: ["Sharper ideas", "Consistent direction", "Better audience fit", "Purposeful execution"],
      process: [
        "We study your brand, audience, competitors, and creative opportunities.",
        "We define the direction, message, pillars, and campaign structure.",
        "We shape concepts and translate them into actionable creative assets.",
        "We refine strategy using audience response and business priorities."
      ],
      related: "Ideas shaped to build emotional brand connection."
    }
  };

  var order = ["social-media", "brand-identity", "content-creation", "paid-campaigns", "website-experience", "creative-strategy"];
  var processLabels = ["Discovery", "Strategy", "Creation", "Optimization"];

  function $(selector, root) {
    return (root || document).querySelector(selector);
  }

  function setText(selector, value) {
    var el = $(selector);
    if (el) {
      el.textContent = value;
    }
  }

  function slugFromPath() {
    var file = window.location.pathname.split("/").pop().replace(".html", "");
    return routes[file] ? file : order.find(function (slug) {
      return routes[slug] && routes[slug].replace(".html", "") === file;
    });
  }

  function getSlug() {
    var params = new URLSearchParams(window.location.search);
    var slug = params.get("service") || document.body.getAttribute("data-service") || slugFromPath() || "social-media";
    return services[slug] ? slug : "social-media";
  }

  function createIcon(className) {
    var icon = document.createElement("i");
    var brandIcons = ["fa-instagram", "fa-youtube", "fa-facebook-f", "fa-linkedin-in", "fa-whatsapp"];
    var prefix = brandIcons.indexOf(className) !== -1 ? "fab" : "fas";
    icon.className = prefix + " " + className;
    icon.setAttribute("aria-hidden", "true");
    return icon;
  }

  function renderPage() {
    var slug = getSlug();
    var service = services[slug];

    document.title = service.title + " Service - AXOMARC";
    document.body.setAttribute("data-service", slug);

    setText("[data-service-title]", service.title.toUpperCase());
    setText("[data-service-headline]", service.headline);
    setText("[data-service-intro]", service.intro);
    setText("[data-service-about]", service.about);
    setText("[data-service-metric-one]", service.metricOne);
    setText("[data-service-metric-one-label]", service.metricOneLabel);
    setText("[data-service-metric-two]", service.metricTwo);
    setText("[data-service-metric-two-label]", service.metricTwoLabel);
    setText("[data-service-why]", service.why);

    document.querySelectorAll("[data-service-start]").forEach(function (link) {
      link.href = "contact.html?service=" + slug;
    });

    var offerGrid = $("[data-offer-grid]");
    if (offerGrid) {
      offerGrid.innerHTML = "";
      service.offers.forEach(function (item, index) {
        var card = document.createElement("article");
        card.className = "axo-service-offer-card axo-service-reveal";
        card.setAttribute("data-delay", String(70 + index * 45));
        card.appendChild(createIcon(item[1]));
        var title = document.createElement("h3");
        title.textContent = item[0];
        var copy = document.createElement("p");
        copy.textContent = item[2];
        card.appendChild(title);
        card.appendChild(copy);
        offerGrid.appendChild(card);
      });
    }

    var proofGrid = $("[data-proof-grid]");
    if (proofGrid) {
      proofGrid.innerHTML = "";
      service.proofs.forEach(function (item) {
        var proof = document.createElement("div");
        proof.className = "axo-service-proof axo-service-reveal";
        proof.appendChild(createIcon("fa-check"));
        var span = document.createElement("span");
        span.textContent = item;
        proof.appendChild(span);
        proofGrid.appendChild(proof);
      });
    }

    var processRow = $("[data-process-row]");
    if (processRow) {
      processRow.innerHTML = "";
      service.process.forEach(function (copy, index) {
        var card = document.createElement("article");
        card.className = "axo-service-process-card axo-service-reveal";
        card.setAttribute("data-delay", String(80 + index * 70));
        var num = document.createElement("span");
        num.textContent = "0" + (index + 1);
        var title = document.createElement("h3");
        title.textContent = processLabels[index];
        var body = document.createElement("p");
        body.textContent = copy;
        card.appendChild(num);
        card.appendChild(title);
        card.appendChild(body);
        processRow.appendChild(card);
      });
    }

    var relatedGrid = $("[data-related-grid]");
    if (relatedGrid) {
      relatedGrid.innerHTML = "";
      order.forEach(function (key) {
        var item = services[key];
        var isActive = key === slug;
        var card = document.createElement(isActive ? "span" : "a");
        card.className = "axo-related-card axo-service-reveal" + (isActive ? " is-active" : "");
        card.setAttribute("data-delay", "80");
        if (isActive) {
          card.setAttribute("aria-current", "page");
        } else {
          card.href = routes[key];
        }
        card.appendChild(createIcon(item.icon));
        var title = document.createElement("h3");
        title.textContent = item.title.toUpperCase();
        var body = document.createElement("p");
        body.textContent = item.related;
        var arrow = document.createElement("span");
        arrow.className = "axo-related-arrow";
        arrow.textContent = isActive ? "Current" : "Open ->";
        card.appendChild(title);
        card.appendChild(body);
        card.appendChild(arrow);
        relatedGrid.appendChild(card);
      });
    }
  }

  function initShell() {
    var body = document.body;
    var preloader = document.getElementById("preloader");
    var nav = document.getElementById("axoNav");
    var mobileToggle = document.getElementById("axoMobileToggle");
    var mobileClose = document.getElementById("axoMobileClose");
    var mobileMenu = document.getElementById("axoMobileMenu");
    var mobileBackdrop = document.getElementById("axoMobileBackdrop");
    var backToTop = document.getElementById("axoBackToTop");

    function finishLoading() {
      body.classList.add("is-loaded");
      if (preloader) {
        window.setTimeout(function () {
          preloader.style.display = "none";
        }, 520);
      }
    }

    window.addEventListener("load", finishLoading);
    window.setTimeout(finishLoading, 1200);

    function updateNav() {
      if (nav) {
        nav.classList.toggle("is-scrolled", window.scrollY > 48);
      }
    }

    updateNav();
    window.addEventListener("scroll", updateNav, { passive: true });

    function toggleMenu(isOpen) {
      if (!mobileMenu) {
        return;
      }
      mobileMenu.classList.toggle("is-active", isOpen);
      body.style.overflow = isOpen ? "hidden" : "";
    }

    if (mobileToggle) {
      mobileToggle.addEventListener("click", function () {
        toggleMenu(true);
      });
    }

    if (mobileClose) {
      mobileClose.addEventListener("click", function () {
        toggleMenu(false);
      });
    }

    if (mobileBackdrop) {
      mobileBackdrop.addEventListener("click", function () {
        toggleMenu(false);
      });
    }

    document.querySelectorAll(".axo-mobile-nav-links a").forEach(function (link) {
      link.addEventListener("click", function () {
        toggleMenu(false);
      });
    });

    if (backToTop) {
      backToTop.addEventListener("click", function () {
        window.scrollTo({ top: 0, behavior: "smooth" });
      });
    }
  }

  function initReveal() {
    var items = document.querySelectorAll(".axo-service-reveal, .axo-footer-reveal");

    items.forEach(function (item) {
      var delay = item.getAttribute("data-delay");
      if (delay) {
        item.style.transitionDelay = delay + "ms";
      }
    });

    if (!("IntersectionObserver" in window)) {
      items.forEach(function (item) {
        item.classList.add("is-visible");
      });
      return;
    }

    var observer = new IntersectionObserver(function (entries) {
      entries.forEach(function (entry) {
        if (entry.isIntersecting) {
          entry.target.classList.add("is-visible");
          observer.unobserve(entry.target);
        }
      });
    }, { threshold: 0.14 });

    items.forEach(function (item) {
      observer.observe(item);
    });
  }

  document.addEventListener("DOMContentLoaded", function () {
    renderPage();
    initShell();
    initReveal();
  });
}());
