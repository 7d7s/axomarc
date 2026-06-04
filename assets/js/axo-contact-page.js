(function () {
  function $(selector, root) {
    return (root || document).querySelector(selector);
  }

  function initPreloader() {
    var body = document.body;
    var preloader = document.getElementById("preloader");

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
  }

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
    var items = document.querySelectorAll(".axo-contact-reveal, .axo-footer-reveal");

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

  function initFaq() {
    document.querySelectorAll("[data-contact-faq]").forEach(function (item, index) {
      var button = item.querySelector("[data-contact-faq-button]");
      if (!button) {
        return;
      }

      if (index === 0) {
        item.classList.add("is-open");
        button.setAttribute("aria-expanded", "true");
      }

      button.addEventListener("click", function () {
        var shouldOpen = !item.classList.contains("is-open");
        document.querySelectorAll("[data-contact-faq]").forEach(function (other) {
          other.classList.remove("is-open");
          var otherButton = other.querySelector("[data-contact-faq-button]");
          if (otherButton) {
            otherButton.setAttribute("aria-expanded", "false");
          }
        });
        item.classList.toggle("is-open", shouldOpen);
        button.setAttribute("aria-expanded", shouldOpen ? "true" : "false");
      });
    });
  }

  function initForm() {
    var form = $("[data-contact-form]");
    var status = $("[data-contact-status]");
    var serviceSelect = $("#service");

    var params = new URLSearchParams(window.location.search);
    var service = params.get("service");
    if (serviceSelect && service) {
      var option = Array.from(serviceSelect.options).find(function (item) {
        return item.value === service;
      });
      if (option) {
        serviceSelect.value = service;
      }
    }

    if (!form) {
      return;
    }

    form.addEventListener("submit", function (event) {
      event.preventDefault();
      var formData = new FormData(form);
      var subject = "AXOMARC Project Inquiry";
      var body = [
        "Full Name: " + (formData.get("fullName") || ""),
        "Email: " + (formData.get("email") || ""),
        "Phone: " + (formData.get("phone") || ""),
        "Company: " + (formData.get("company") || ""),
        "Service: " + (formData.get("service") || ""),
        "Budget: " + (formData.get("budget") || ""),
        "Legal Agreement: Accepted Terms & Conditions and Disclaimer",
        "",
        "Message:",
        formData.get("message") || ""
      ].join("\n");

      if (status) {
        status.textContent = "Opening your email app with the project details.";
      }

      window.location.href = "mailto:hello@axomarc.com?subject=" + encodeURIComponent(subject) + "&body=" + encodeURIComponent(body);
    });
  }

  document.addEventListener("DOMContentLoaded", function () {
    initPreloader();
    initNav();
    initReveal();
    initFaq();
    initForm();
  });
}());
