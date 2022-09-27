using System;
using System.Collections.Generic;
using System.Text;
using System.Configuration;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.Configuration.Json;

namespace s3b
{
    public class Config
    {
        static IConfiguration configBuilder = null;

        protected static Model _settings = null;
        protected static Template _template = null;

        static IConfiguration getConfigBuilder()
        {
            if (configBuilder == null)
            {
                configBuilder = new ConfigurationBuilder()
                .AddJsonFile("appsettings.json", true, true)
                .Build();
            }

            return configBuilder;
        }
        static public string getString(string k)
        {
            Model settings = getSettings();
            string result = string.Empty;

            if (settings.ContainsKey(k))
            {
                object o = settings[k];

                if (o != null)
                {
                    result = o.ToString();

                    Template t = getTemplate();

                    result = t.eval(result, settings);
                }
            }

            return result;
        }

        static private string getConfigString(string param)
        {
            IConfigurationSection section = getConfigBuilder().GetSection("appsettings");

            return section[param];
        }

        protected static Template getTemplate()
        {
            if (_template == null)
            {
                _template = new Template();
            }

            return _template;
        }

        static public void setValue(string k, string v)
        {
            Model settings = getSettings();

            if (settings.ContainsKey(k))
            {
                settings[k] = v;
            }
            else
            {
                settings.Add(k, v);
            }
        }

        static public void setValue(string k, int v)
        {
            Model settings = getSettings();

            if (settings.ContainsKey(k))
            {
                settings[k] = v.ToString();
            }
            else
            {
                settings.Add(k, v.ToString());
            }

        }


        static public int getInt(string k)
        {
            Model settings = getSettings();
            int result = 0;

            if (settings.ContainsKey(k))
            {
                result = Convert.ToInt32(settings[k]);
            }

            return result;
        }

        static public Model getSettings()
        {
            if (_settings == null)
            {
                _settings = new Model();

                IConfigurationSection section = getConfigBuilder().GetSection("appsettings");

                foreach (var c in section.GetChildren())
                {
                    string k = c.Key;
                    string v = getConfigString(k);

                    _settings.Add(k, v);
                }
            }

            return _settings;

        }
    }
}
