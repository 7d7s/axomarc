(function () {
  function initNav() {
    var body = document.body;
    var nav = document.getElementById("axoNav");
    var mobileToggle = document.getElementById("axoMobileToggle");
    var mobileClose = document.getElementById("axoMobileClose");
    var mobileMenu = document.getElementById("axoMobileMenu");
    var mobileBackdrop = document.getElementById("axoMobileBackdrop");
    var backToTop = document.getElementById("axoBackToTop");

    function updateNav() {
      if (nav) {
        nav.classList.toggle("is-scrolled", window.scrollY > 48);
      }
    }

    function toggleMenu(isOpen) {
      if (!mobileMenu) {
        return;
      }
      mobileMenu.classList.toggle("is-active", isOpen);
      body.style.overflow = isOpen ? "hidden" : "";
    }

    updateNav();
    window.addEventListener("scroll", updateNav, { passive: true });

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
    var items = document.querySelectorAll(".axo-legal-reveal, .axo-footer-reveal");

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

  function initAccordion() {
    document.querySelectorAll("[data-legal-accordion]").forEach(function (item, index) {
      var button = item.querySelector("[data-legal-accordion-button]");
      if (!button) {
        return;
      }

      if (index === 0) {
        item.classList.add("is-open");
        button.setAttribute("aria-expanded", "true");
      }

      button.addEventListener("click", function () {
        var shouldOpen = !item.classList.contains("is-open");
        document.querySelectorAll("[data-legal-accordion]").forEach(function (other) {
          other.classList.remove("is-open");
          var otherButton = other.querySelector("[data-legal-accordion-button]");
          if (otherButton) {
            otherButton.setAttribute("aria-expanded", "false");
          }
        });
        item.classList.toggle("is-open", shouldOpen);
        button.setAttribute("aria-expanded", shouldOpen ? "true" : "false");
      });
    });
  }

  document.addEventListener("DOMContentLoaded", function () {
    initNav();
    initReveal();
    initAccordion();
  });
}());
